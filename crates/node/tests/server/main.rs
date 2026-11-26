//! The REST route tests that share the compat-index node in `support`, built as
//! one test binary so a rebuild links once. Each module keeps the features it
//! needed as its own test target.

#[cfg(feature = "reader")]
#[path = "../support/mod.rs"]
mod support;

#[cfg(all(feature = "coordinator", feature = "reader", feature = "writer"))]
mod alias_route;
#[cfg(all(feature = "reader", feature = "writer"))]
pub mod assemble;
#[cfg(all(feature = "coordinator", feature = "reader", feature = "writer"))]
mod boot;
#[cfg(all(feature = "coordinator", feature = "reader", feature = "writer"))]
mod boot_buckets;
#[cfg(all(feature = "coordinator", feature = "reader", feature = "writer"))]
mod boot_give_back;
mod boot_grpc;
mod boot_grpc_cost;
mod boot_h3_cost;
#[cfg(all(feature = "coordinator", feature = "reader", feature = "writer"))]
mod boot_lease_load;
#[cfg(all(feature = "coordinator", feature = "reader", feature = "writer"))]
mod boot_limit;
mod boot_page_cache;
#[cfg(all(feature = "coordinator", feature = "reader", feature = "writer"))]
mod boot_ready_drain;
mod boot_retire;
#[cfg(all(feature = "coordinator", feature = "reader", feature = "writer"))]
mod boot_roles;
mod boot_rollover_count;
mod boot_search_queue;
mod boot_search_slots;
#[cfg(all(feature = "coordinator", feature = "reader", feature = "writer"))]
mod boot_snapshots;
mod boot_templates_ingest;
#[cfg(all(feature = "coordinator", feature = "reader", feature = "writer"))]
mod boot_usage;
mod boot_warm_join;
#[cfg(all(feature = "coordinator", feature = "reader", feature = "writer"))]
mod boot_write_concurrency;
#[cfg(all(feature = "reader", feature = "writer"))]
mod bulk_route;
#[cfg(all(feature = "coordinator", feature = "reader", feature = "writer"))]
mod by_query_route;
#[cfg(all(feature = "reader", feature = "writer"))]
mod cluster_assemble;
#[cfg(all(feature = "coordinator", feature = "reader", feature = "writer"))]
mod data_stream_route;
#[cfg(all(feature = "reader", feature = "writer"))]
mod doc_route;
#[cfg(all(feature = "reader", feature = "writer"))]
mod get_route;
#[cfg(all(feature = "coordinator", feature = "reader", feature = "writer"))]
mod index_delete;
#[cfg(all(feature = "coordinator", feature = "reader", feature = "writer"))]
mod index_pins;
#[cfg(all(feature = "coordinator", feature = "reader", feature = "writer"))]
mod index_route;
#[cfg(all(feature = "reader", feature = "writer"))]
mod ingest_run;
#[cfg(all(feature = "coordinator", feature = "reader", feature = "writer"))]
mod metadata_block;
#[cfg(feature = "reader")]
mod newest_ask;
#[cfg(all(feature = "extensions", feature = "reader", feature = "writer"))]
mod pinned_read;
#[cfg(all(feature = "reader", feature = "writer"))]
mod pipeline_assemble;
#[cfg(all(feature = "reader", feature = "writer"))]
mod pipeline_route;
#[cfg(feature = "reader")]
mod push_read;
#[cfg(all(feature = "reader", feature = "writer"))]
mod read_block;
#[cfg(all(feature = "reader", feature = "writer"))]
mod read_forward;
mod restore_flags;
mod restore_settings;
#[cfg(all(feature = "coordinator", feature = "reader", feature = "writer"))]
mod rollover_route;
mod rollover_shape;
mod rustfs_rig;
#[cfg(all(feature = "reader", feature = "writer"))]
mod script_route;
#[cfg(all(feature = "coordinator", feature = "reader", feature = "writer"))]
mod search_default_pipeline_setting;
#[cfg(all(feature = "reader", feature = "writer"))]
mod search_pipeline_route;
#[cfg(all(feature = "reader", feature = "writer"))]
mod security_authn;
#[cfg(all(feature = "reader", feature = "writer"))]
mod security_authz;
mod security_hardening;
#[cfg(all(feature = "reader", feature = "writer"))]
mod security_limits;
mod security_route;
#[cfg(all(feature = "coordinator", feature = "reader", feature = "writer"))]
mod settings_route;
mod snapshot_delete;
mod snapshot_global_race;
mod snapshot_global_restore;
mod snapshot_global_take;
mod snapshot_partial_restore;
mod snapshot_partial_take;
mod snapshot_restore;
mod snapshot_restore_aliases;
mod snapshot_restore_background;
mod snapshot_restore_bounds;
mod snapshot_restore_compat;
mod snapshot_restore_search;
#[cfg(all(feature = "reader", feature = "writer"))]
mod snapshot_route;
mod snapshot_take;
mod snapshot_take_size;
mod stock_export;
mod stock_export_budget;
mod stock_restore;
mod stock_restore_budget;
mod stock_restore_search;
mod stock_restore_settings;
mod stored_search_template;
#[cfg(all(feature = "reader", feature = "writer"))]
mod template_route;
