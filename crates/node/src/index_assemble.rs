//! The index service a node serves creates with: descriptors that record
//! their changes, the bucket join and leave, and the template sets when the
//! setting is on.

use std::sync::Arc;

use oss_blob::BlobStore;
use oss_meta::{DescriptorStore, PitClock};

use crate::index_delete::{DeleteService, NoSnapshots};
use crate::index_route::IndexService;
use crate::index_templates::TemplateSets;
use crate::ingest_run::IngestPipelines;

pub(crate) fn index_service(
    config: &crate::assemble::Config,
    store: &Arc<dyn BlobStore>,
    clock: &PitClock,
    bucket_wiring: Option<&Arc<crate::writer::BucketWiring>>,
    stripes: oss_meta::ChangeStripes,
) -> Result<Arc<IndexService>, crate::assemble::AssembleError> {
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
    Ok(Arc::new(indices.with_knn(config.knn)))
}

pub(crate) fn delete_service(
    config: &crate::assemble::Config,
    store: &Arc<dyn BlobStore>,
    clock: &PitClock,
) -> Result<Arc<DeleteService>, crate::assemble::AssembleError> {
    DeleteService::new(
        store,
        &config.base,
        config.delete_mode,
        Arc::clone(clock),
        Arc::new(NoSnapshots),
        config.pit_buffer_bytes,
        config.inline_purge_max_shards,
    )
    .map(Arc::new)
    .map_err(|e| crate::assemble::AssembleError::Store(e.to_string()))
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
