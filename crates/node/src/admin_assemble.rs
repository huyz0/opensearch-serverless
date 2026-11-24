//! The cluster settings and index table routes a node always serves, and the
//! template routes when templates are on: they
//! read the registers under the deployment's base and need nothing else.

use std::sync::Arc;

use oss_blob::BlobStore;
use oss_meta::DescriptorStore;
use oss_rest::Router;

use crate::assemble::{AssembleError, Config};
use crate::cat_route::{self, CatService};
use crate::cluster_route::{self, ClusterService};
use crate::pipeline_route::{self, PipelineService};
use crate::template_route::{self, TemplateService};

/// Registers `GET`/`PUT /_cluster/settings` and the `_cat` and `_list` index
/// tables on `router`, over `store`.
pub(crate) fn register(
    router: Router,
    store: &Arc<dyn BlobStore>,
    config: &Config,
) -> Result<Router, AssembleError> {
    let store_error = |e: oss_meta::MetaError| AssembleError::Store(e.to_string());
    let settings = ClusterService::new(Arc::clone(store), &config.base, config.publish_tick_millis)
        .map_err(store_error)?;
    let descriptors = DescriptorStore::new(Arc::clone(store), &config.base, config.delete_mode)
        .map_err(store_error)?;
    let router = cluster_route::routes(router, Arc::new(settings)).map_err(AssembleError::Route)?;
    let router = cat_route::routes(router, Arc::new(CatService::new(descriptors)))
        .map_err(AssembleError::Route)?;
    let router = if config.index_templates {
        let templates = TemplateService::new(store, &config.base, config.pit_buffer_bytes)
            .map_err(store_error)?;
        template_route::routes(router, &Arc::new(templates)).map_err(AssembleError::Route)?
    } else {
        router
    };
    if !config.ingest_pipelines {
        return Ok(router);
    }
    let pipelines =
        PipelineService::new(store, &config.base, config.pit_buffer_bytes).map_err(store_error)?;
    pipeline_route::routes(router, &Arc::new(pipelines)).map_err(AssembleError::Route)
}
