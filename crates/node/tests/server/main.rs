//! The REST route tests that share the compat-index node in `support`, built as
//! one test binary so a rebuild links once. Each module keeps the features it
//! needed as its own test target.

#[cfg(feature = "reader")]
#[path = "../support/mod.rs"]
mod support;

#[cfg(all(feature = "reader", feature = "writer"))]
mod assemble;
#[cfg(all(feature = "coordinator", feature = "reader", feature = "writer"))]
mod boot;
#[cfg(all(feature = "coordinator", feature = "reader", feature = "writer"))]
mod boot_buckets;
#[cfg(all(feature = "coordinator", feature = "reader", feature = "writer"))]
mod boot_give_back;
#[cfg(all(feature = "coordinator", feature = "reader", feature = "writer"))]
mod boot_lease_load;
#[cfg(all(feature = "coordinator", feature = "reader", feature = "writer"))]
mod boot_limit;
#[cfg(all(feature = "coordinator", feature = "reader", feature = "writer"))]
mod boot_usage;
#[cfg(all(feature = "reader", feature = "writer"))]
mod bulk_route;
#[cfg(all(feature = "coordinator", feature = "reader", feature = "writer"))]
mod by_query_route;
#[cfg(all(feature = "reader", feature = "writer"))]
mod cluster_assemble;
#[cfg(all(feature = "reader", feature = "writer"))]
mod doc_route;
#[cfg(all(feature = "reader", feature = "writer"))]
mod get_route;
#[cfg(all(feature = "coordinator", feature = "reader", feature = "writer"))]
mod index_delete;
#[cfg(all(feature = "coordinator", feature = "reader", feature = "writer"))]
mod index_route;
#[cfg(feature = "reader")]
mod newest_ask;
#[cfg(all(feature = "reader", feature = "writer"))]
mod pipeline_route;
#[cfg(feature = "reader")]
mod push_read;
#[cfg(all(feature = "reader", feature = "writer"))]
mod read_forward;
#[cfg(all(feature = "reader", feature = "writer"))]
mod template_route;
