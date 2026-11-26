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
) -> Result<(Router, Option<Arc<SecurityState>>), AssembleError> {
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
            Some(Arc::new(
                SnapshotTakeService::new(
                    Arc::clone(store),
                    &config.base,
                    descriptors,
                    Arc::new(crate::clock::now_millis),
                )
                .with_system_indices(system_indices(config))
                .with_capture_aliases(config.snapshots_capture_aliases)
                .with_partial_shards(config.snapshots_partial_shards)
                .with_capture_global_state(config.snapshots_capture_global_state)
                .with_stock_export(
                    crate::stock_export_route::StockExportSettings {
                        enabled: config.snapshots_stock_export,
                        roots: oss_blob::SourceRoots::parse(
                            &config.stock_export_roots.0,
                            &config.stock_export_roots.1,
                        ),
                        s3: config.stock_s3.clone().map(|s3| (s3, config.base.clone())),
                        chunk_bytes: crate::stock_export_route::EXPORT_CHUNK_BYTES,
                    },
                ),
            ))
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
    let (router, state) = if config.security {
        let (router, state) = security(router, store, config, budget)?;
        (router, Some(state))
    } else {
        (router, None)
    };
    let router = extensions(router, store, config, budget)?;
    let router = plugin_routes(config, router)?;
    let scripts =
        crate::script_route::ScriptService::new(store, &config.base, config.pit_buffer_bytes)
            .map_err(store_error)?;
    let router =
        crate::script_route::routes(router, &Arc::new(scripts)).map_err(AssembleError::Route)?;
    let router = if config.search_pipelines {
        let searching = crate::search_pipeline_route::SearchPipelines::open(
            store,
            &config.base,
            oss_format::MAX_DOC_BYTES,
        )
        .map_err(store_error)?;
        crate::search_pipeline_route::routes(router, &searching).map_err(AssembleError::Route)?
    } else {
        router
    };
    if !config.ingest_pipelines {
        return Ok((router, state));
    }
    let pipelines =
        PipelineService::new(store, &config.base, config.pit_buffer_bytes).map_err(store_error)?;
    let router =
        pipeline_route::routes(router, &Arc::new(pipelines)).map_err(AssembleError::Route)?;
    Ok((router, state))
}

/// The module upload route, when the node is built with extensions and has a
/// pool for them; without either, nothing is loaded and the route does not exist.
#[cfg(feature = "extensions")]
fn extensions(
    router: Router,
    store: &Arc<dyn BlobStore>,
    config: &Config,
    budget: &MemoryBudget,
) -> Result<Router, AssembleError> {
    if budget.limit(oss_mem::Pool::Extensions) == 0 {
        return Ok(router);
    }
    let host = crate::extension_host::host();
    let service = crate::extension_route::ExtensionService::new(
        Arc::clone(store),
        &config.base,
        host,
        budget.clone(),
    );
    crate::extension_route::routes(router, Arc::new(service)).map_err(AssembleError::Route)
}

/// Without the `extensions` feature the setting is ignored.
#[cfg(not(feature = "extensions"))]
fn extensions(
    router: Router,
    _store: &Arc<dyn BlobStore>,
    _config: &Config,
    _budget: &MemoryBudget,
) -> Result<Router, AssembleError> {
    Ok(router)
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

/// Gives the search service the pipelines `?search_pipeline=` names, when
/// the setting is on.
pub(crate) fn search_pipelines<S: BlobStore + 'static>(
    config: &crate::assemble::Config,
    store: &Arc<dyn BlobStore>,
    search: &crate::search::SearchService<S>,
) -> Result<(), crate::assemble::AssembleError> {
    if config.search_pipelines {
        let pipelines = crate::search_pipeline_route::SearchPipelines::open(
            store,
            &config.base,
            oss_format::MAX_DOC_BYTES,
        )
        .map_err(|e| crate::assemble::AssembleError::Store(e.to_string()))?;
        search.set_search_pipelines(pipelines);
    }
    Ok(())
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
) -> Result<(Router, Arc<SecurityState>), AssembleError> {
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
    let guard = AuthGuard(Arc::new(Authenticator::new(Arc::clone(&state), clock)));
    let router = router
        .with_guard(Arc::new(guard))
        .with_authorizer(Arc::new(authorizer));
    Ok((router, state))
}

/// The names a take and a restore refuse, as the node is configured.
pub(crate) fn system_indices(
    config: &crate::assemble::Config,
) -> Arc<oss_coordinator::system_indices::SystemIndices> {
    Arc::new(oss_coordinator::system_indices::SystemIndices::parse(
        &config.system_index_patterns,
    ))
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

    // docs/specs/snapshot-restore AC-2
    #[test]
    fn the_configured_patterns_are_the_system_indices() {
        let config = Config {
            system_index_patterns: ".kibana*, .tasks".to_owned(),
            ..Config::new("", "n", [0; 6])
        };
        let system = system_indices(&config);
        assert!(system.contains(".kibana_1") && system.contains(".tasks"));
        assert!(!system.contains("logs"));
        assert!(!system_indices(&Config::new("", "n", [0; 6])).contains(".kibana_1"));
    }

    // docs/specs/extension-abi AC-8
    #[cfg(feature = "extensions")]
    #[test]
    fn the_upload_route_exists_only_with_an_extensions_pool() {
        let taken = |budget: &MemoryBudget| {
            let (store, _) = parts();
            let config = Config::new("", "n", [0; 6]);
            let router = extensions(Router::new(), &store, &config, budget).expect("assembled");
            let service = crate::extension_route::ExtensionService::new(
                Arc::clone(&store),
                "",
                crate::extension_host::host(),
                budget.clone(),
            );
            crate::extension_route::routes(router, Arc::new(service)).is_err()
        };
        let (_, off) = parts();
        assert!(!taken(&off), "no pool, no route");
        let on = MemoryBudget::new(
            &BudgetConfig::new(64 << 20)
                .with_pool(Pool::Extensions, 16 << 20)
                .expect("fits"),
        );
        assert!(taken(&on), "a pool, the route");
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
        let (router, _) = register(Router::new(), &store, &config, &budget).expect("registers");
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
        let (router, _) = register(Router::new(), &store, &config, &budget).expect("registers");
        assert!(router.has_guard());
        assert!(router.has_authorizer());
    }
}
