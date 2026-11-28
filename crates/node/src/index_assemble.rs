//! The index service a node serves creates with: descriptors that record
//! their changes, the bucket join and leave, and the template sets when the
//! setting is on.

use std::sync::Arc;

use oss_blob::BlobStore;
use oss_meta::{DescriptorStore, PitClock};

use crate::index_delete::{DeleteService, NoSnapshots, SnapshotPins, StoreSnapshotPins};
use crate::index_route::IndexService;
use crate::index_templates::TemplateSets;

pub(crate) fn index_service(
    config: &crate::assemble::Config,
    store: &Arc<dyn BlobStore>,
    clock: &PitClock,
    bucket_wiring: Option<&Arc<crate::writer::BucketWiring>>,
    pins: Option<Arc<dyn crate::pin_policy::PinPolicy>>,
    events: &crate::lifecycle_bus::Bus,
) -> Result<Arc<IndexService>, crate::assemble::AssembleError> {
    // Every update and delete is recorded in the change stripes, which is
    // what lets a writer confirm an index it kept without reading it.
    let stripes = oss_meta::ChangeStripes::new(Arc::clone(store), &config.base)
        .map_err(|e| crate::assemble::AssembleError::Store(e.to_string()))?;
    let descriptors = DescriptorStore::new(Arc::clone(store), &config.base, config.delete_mode)
        .map_err(|e| crate::assemble::AssembleError::Store(e.to_string()))?
        .record_changes_in(stripes);
    let mut indices = IndexService::new(descriptors, Arc::clone(clock), config.pit_buffer_bytes);
    if let Some(buckets) = bucket_wiring {
        let (joiner, leaver) = (Arc::clone(buckets), Arc::clone(buckets));
        indices = indices.with_bucket_join(
            Arc::new(move |group, name, uuid| joiner.join(group, name, uuid)),
            Arc::new(move |bucket, uuid| leaver.leave(bucket, uuid)),
        );
    }
    if config.index_templates {
        let sets = TemplateSets::open(store, &config.base)
            .map_err(|e| crate::assemble::AssembleError::Store(e.to_string()))?;
        indices = indices.with_templates(sets);
    }
    if let Some(policy) = pins {
        indices = indices.with_pins(policy);
    }
    #[cfg(all(feature = "reader", feature = "writer"))]
    if config.ism {
        let managed = DescriptorStore::new(Arc::clone(store), &config.base, config.delete_mode)
            .map_err(|e| crate::assemble::AssembleError::Store(e.to_string()))?;
        let ism = crate::ism_route::IsmService::new(store, &config.base, config.pit_buffer_bytes)
            .with_descriptors(managed);
        indices = indices.with_created(Arc::new(ism).on_create());
    }
    Ok(Arc::new(
        indices
            .with_events(events.clone())
            .with_knn(config.knn)
            .with_nested(config.nested)
            .with_geo(config.geo)
            .with_pipelines(config.ingest_pipelines)
            .with_search_pipelines(config.search_pipelines),
    ))
}

/// The policy a create's pins are checked by: the module store, the
/// `all_access` role (every caller when security is off) and the cluster
/// setting `serverless.extensions.pins.allowed`. None, so that every pin is
/// refused, when the node is built without extensions or has no pool for
/// them.
#[cfg(feature = "extensions")]
pub(crate) fn pin_policy(
    config: &crate::assemble::Config,
    store: &Arc<dyn BlobStore>,
    budget: &oss_mem::MemoryBudget,
    security: Option<Arc<crate::security_state::SecurityState>>,
) -> Result<Option<Arc<dyn crate::pin_policy::PinPolicy>>, crate::assemble::AssembleError> {
    if budget.limit(oss_mem::Pool::Extensions) == 0 {
        return Ok(None);
    }
    let admin: crate::extension_pins::Admin = match security {
        // Security off: every caller is the administrator.
        None => Arc::new(|_| true),
        Some(state) => Arc::new(move |user| {
            user.is_some_and(|u| {
                state
                    .config()
                    .is_ok_and(|c| c.all_access(crate::security_who::who(u)))
            })
        }),
    };
    let policy = crate::extension_pins::ModulePins::new(
        Arc::clone(store),
        &config.base,
        crate::extension_host::host(),
        budget.clone(),
        admin,
    )
    .gated_by_the_cluster_setting()
    .map_err(|e| crate::assemble::AssembleError::Store(e.to_string()))?;
    Ok(Some(Arc::new(policy)))
}

/// Without the `extensions` feature no pin is allowed.
#[cfg(not(feature = "extensions"))]
pub(crate) fn pin_policy(
    _config: &crate::assemble::Config,
    _store: &Arc<dyn BlobStore>,
    _budget: &oss_mem::MemoryBudget,
    _security: Option<Arc<crate::security_state::SecurityState>>,
) -> Result<Option<Arc<dyn crate::pin_policy::PinPolicy>>, crate::assemble::AssembleError> {
    Ok(None)
}

pub(crate) fn delete_service(
    config: &crate::assemble::Config,
    store: &Arc<dyn BlobStore>,
    clock: &PitClock,
    bucket_wiring: Option<&Arc<crate::writer::BucketWiring>>,
    events: &crate::lifecycle_bus::Bus,
) -> Result<Arc<DeleteService>, crate::assemble::AssembleError> {
    // With the snapshot routes on, a delete reads the snapshots before it
    // purges: a shallow snapshot names the index's blobs and copies none.
    let snapshots: Arc<dyn SnapshotPins> = if config.snapshots {
        Arc::new(StoreSnapshotPins::new(
            Arc::clone(store),
            &config.base,
            config.janitor_max_views,
            config.pit_buffer_bytes,
        ))
    } else {
        Arc::new(NoSnapshots)
    };
    DeleteService::new(
        store,
        &config.base,
        config.delete_mode,
        Arc::clone(clock),
        snapshots,
        config.pit_buffer_bytes,
        config.inline_purge_max_shards,
    )
    .map(|service| match bucket_wiring {
        Some(buckets) => {
            let leaver = Arc::clone(buckets);
            service.with_bucket_leave(Arc::new(move |bucket, uuid| leaver.leave(bucket, uuid)))
        }
        None => service,
    })
    .map(|service| service.with_events(events.clone()))
    .map(Arc::new)
    .map_err(|e| crate::assemble::AssembleError::Store(e.to_string()))
}

/// The restore route, answering 501 until `serverless.snapshots.restore.enabled`
/// (with the take's switches) is on.
pub(crate) fn restore_routes(
    router: oss_rest::Router,
    config: &crate::assemble::Config,
    store: &Arc<dyn BlobStore>,
    clock: &PitClock,
    deletes: &Arc<DeleteService>,
    budget: &oss_mem::MemoryBudget,
) -> Result<
    (
        oss_rest::Router,
        Option<Arc<crate::restore_background::BackgroundRestores>>,
    ),
    crate::assemble::AssembleError,
> {
    if !config.snapshots {
        return Ok((router, None));
    }
    let restores = crate::restore_background::BackgroundRestores::new(
        config.restore_background,
        budget.clone(),
    );
    let descriptors = DescriptorStore::new(Arc::clone(store), &config.base, config.delete_mode)
        .map_err(|e| crate::assemble::AssembleError::Store(e.to_string()))?;
    let service = crate::snapshot_restore_route::SnapshotRestoreService::new(
        Arc::clone(store),
        &config.base,
        descriptors,
        Arc::clone(deletes),
        Arc::clone(clock),
        config.snapshots_take && config.snapshots_restore,
        crate::snapshot_restore_route::COPY_CHUNK_BYTES,
    )
    .with_background(Arc::clone(&restores))
    .with_alias_buffers(config.pit_buffer_bytes, ALIAS_LEASE_TTL_MILLIS)
    .with_system_indices(crate::admin_assemble::system_indices(config))
    .with_stock(crate::stock_restore_route::StockRestoreSettings {
        enabled: config.snapshots_stock,
        roots: oss_blob::SourceRoots::parse(&config.stock_roots.0, &config.stock_roots.1),
        fanout: config.stock_fanout,
        s3: config
            .stock_s3
            .clone()
            .map(|s3| crate::stock_restore_route::StockS3 {
                source: s3,
                own_prefix: config.base.clone(),
            }),
    });
    crate::snapshot_restore_route::routes(router, Arc::new(service))
        .map(|router| (router, Some(restores)))
        .map_err(crate::assemble::AssembleError::Route)
}

/// How long an alias change's reservation holds before another writer may
/// undo it.
const ALIAS_LEASE_TTL_MILLIS: i64 = 30_000;

/// The alias routes over the same descriptors and change stripes as the
/// index routes.
pub(crate) fn alias_service(
    config: &crate::assemble::Config,
    store: &Arc<dyn BlobStore>,
    clock: &PitClock,
) -> Result<Arc<crate::alias_route::AliasService>, crate::assemble::AssembleError> {
    let stripes = oss_meta::ChangeStripes::new(Arc::clone(store), &config.base)
        .map_err(|e| crate::assemble::AssembleError::Store(e.to_string()))?;
    let descriptors = DescriptorStore::new(Arc::clone(store), &config.base, config.delete_mode)
        .map_err(|e| crate::assemble::AssembleError::Store(e.to_string()))?
        .record_changes_in(stripes);
    Ok(Arc::new(crate::alias_route::AliasService::new(
        descriptors,
        Arc::clone(clock),
        config.pit_buffer_bytes,
        ALIAS_LEASE_TTL_MILLIS,
    )))
}

/// The settings update route over the same descriptors and change stripes as
/// the index routes.
pub(crate) fn settings_routes(
    router: oss_rest::Router,
    config: &crate::assemble::Config,
    store: &Arc<dyn BlobStore>,
    role: &Arc<crate::writer::WriterRole>,
) -> Result<oss_rest::Router, crate::assemble::AssembleError> {
    let stripes = oss_meta::ChangeStripes::new(Arc::clone(store), &config.base)
        .map_err(|e| crate::assemble::AssembleError::Store(e.to_string()))?;
    let descriptors = DescriptorStore::new(Arc::clone(store), &config.base, config.delete_mode)
        .map_err(|e| crate::assemble::AssembleError::Store(e.to_string()))?
        .record_changes_in(stripes);
    let service = Arc::new(
        crate::settings_route::SettingsService::new(
            descriptors,
            config.pit_buffer_bytes,
            config.ingest_pipelines,
        )
        .with_search_pipelines(config.search_pipelines)
        .with_role(Arc::clone(role)),
    );
    crate::settings_route::routes(router, &service).map_err(crate::assemble::AssembleError::Route)
}

/// Adds the settings update, data stream and rollover routes: backing indices are made by `indices` and
/// removed by `deletes`.
pub(crate) fn stream_routes<S: BlobStore + 'static>(
    router: oss_rest::Router,
    config: &crate::assemble::Config,
    store: &Arc<dyn BlobStore>,
    clock: &PitClock,
    indices: &Arc<IndexService>,
    deletes: &Arc<DeleteService>,
    search: &Arc<crate::search::SearchService<S>>,
) -> Result<oss_rest::Router, crate::assemble::AssembleError> {
    let descriptors = || {
        let stripes = oss_meta::ChangeStripes::new(Arc::clone(store), &config.base)
            .map_err(|e| crate::assemble::AssembleError::Store(e.to_string()))?;
        DescriptorStore::new(Arc::clone(store), &config.base, config.delete_mode)
            .map(|d| d.record_changes_in(stripes))
            .map_err(|e| crate::assemble::AssembleError::Store(e.to_string()))
    };
    let rollover = Arc::new(
        crate::rollover_route::RolloverService::new(
            Arc::clone(indices),
            Arc::clone(deletes),
            descriptors()?,
            Arc::clone(clock),
            config.pit_buffer_bytes,
            ALIAS_LEASE_TTL_MILLIS,
        )
        .with_counter(Arc::new(crate::rollover_route::SearchCount::new(
            Arc::clone(search),
        ))),
    );
    let service = Arc::new(crate::data_stream_route::DataStreamService::new(
        Arc::clone(indices),
        Arc::clone(deletes),
        descriptors()?,
        Arc::clone(clock),
        config.pit_buffer_bytes,
        ALIAS_LEASE_TTL_MILLIS,
    ));
    let router = crate::data_stream_route::routes(router, service)
        .map_err(crate::assemble::AssembleError::Route)?;
    crate::rollover_route::routes(router, rollover).map_err(crate::assemble::AssembleError::Route)
}

#[cfg(all(test, feature = "extensions"))]
mod tests {
    use oss_blob::Emulator;
    use oss_mem::{BudgetConfig, MemoryBudget, Pool};

    use super::*;
    use crate::assemble::Config;

    fn budget(extensions: usize) -> MemoryBudget {
        let mut c = BudgetConfig::new(256 << 20)
            .with_pool(Pool::RequestBodies, 32 << 20)
            .expect("fits");
        if extensions > 0 {
            c = c.with_pool(Pool::Extensions, extensions).expect("fits");
        }
        MemoryBudget::new(&c)
    }

    fn store() -> Arc<dyn BlobStore> {
        Arc::new(Emulator::new(1 << 24, 1))
    }

    // docs/specs/extension-analyzer AC-14
    #[test]
    fn a_node_with_no_extensions_pool_has_no_pin_policy() {
        let config = Config::new("", "n", [0; 6]);
        let none = pin_policy(&config, &store(), &budget(0), None).expect("built");
        assert!(none.is_none());
        let some = pin_policy(&config, &store(), &budget(16 << 20), None).expect("built");
        assert!(some.is_some());
    }

    // docs/specs/extension-analyzer AC-9
    #[test]
    fn with_security_off_every_caller_may_pin_and_the_cluster_setting_is_the_gate() {
        let config = Config::new("", "n", [0; 6]);
        let policy = pin_policy(&config, &store(), &budget(16 << 20), None)
            .expect("built")
            .expect("a policy");
        assert!(policy.authorised(None));
        assert!(
            policy.allowed().is_err(),
            "nothing is written to the cluster register: pins are off"
        );
    }
}
