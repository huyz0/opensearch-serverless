//! The cluster settings and index table routes a node always serves: they
//! read the registers under the deployment's base and need nothing else.

use std::sync::Arc;

use oss_blob::BlobStore;
use oss_meta::DescriptorStore;
use oss_rest::Router;

use crate::assemble::{AssembleError, Config};
use crate::cat_route::{self, CatService};
use crate::cluster_route::{self, ClusterService};

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
    cat_route::routes(router, Arc::new(CatService::new(descriptors))).map_err(AssembleError::Route)
}
