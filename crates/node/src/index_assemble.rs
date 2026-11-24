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
    Ok(Arc::new(
        indices
            .with_knn(config.knn)
            .with_pipelines(config.ingest_pipelines),
    ))
}

pub(crate) fn delete_service(
    config: &crate::assemble::Config,
    store: &Arc<dyn BlobStore>,
    clock: &PitClock,
    bucket_wiring: Option<&Arc<crate::writer::BucketWiring>>,
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
) -> Result<oss_rest::Router, crate::assemble::AssembleError> {
    if !config.snapshots {
        return Ok(router);
    }
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
    .with_system_indices(crate::admin_assemble::system_indices(config));
    crate::snapshot_restore_route::routes(router, Arc::new(service))
        .map_err(crate::assemble::AssembleError::Route)
}
