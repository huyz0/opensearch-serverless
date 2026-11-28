//! The node binary's composition root.
//!
//! Each role is compiled in only when its build switch (a Cargo feature) is
//! on, so a reader-only build carries no writer code. [`enabled_roles`] reports
//! what this binary was built with.

#[cfg(all(feature = "reader", feature = "writer"))]
pub mod ad_route;
pub mod ad_setting;
#[cfg(all(feature = "reader", feature = "writer"))]
mod admin_assemble;
#[cfg(all(feature = "reader", feature = "writer"))]
pub mod alerting_route;
#[cfg(all(feature = "reader", feature = "writer"))]
pub mod alerting_run;
#[cfg(all(feature = "writer", feature = "coordinator"))]
pub mod alias_route;
#[cfg(all(feature = "reader", feature = "writer"))]
pub mod assemble;
#[cfg(all(feature = "reader", feature = "writer"))]
mod assemble_config;
#[cfg(all(feature = "reader", feature = "writer"))]
mod assemble_error;
#[cfg(all(feature = "reader", feature = "writer"))]
mod assemble_publishing;
#[cfg(all(feature = "reader", feature = "writer"))]
mod assemble_writer;
#[cfg(feature = "reader")]
pub mod async_search;
pub mod async_search_setting;
#[cfg(all(feature = "reader", feature = "writer"))]
pub mod boot;
#[cfg(feature = "writer")]
pub mod bulk_handler;
#[cfg(feature = "writer")]
pub mod bulk_route;
#[cfg(all(feature = "reader", feature = "writer"))]
mod bulk_sink;
#[cfg(all(feature = "writer", feature = "coordinator"))]
pub mod by_query;
#[cfg(feature = "writer")]
pub mod cat_route;
#[cfg(all(feature = "reader", feature = "writer"))]
mod clock;
#[cfg(all(feature = "reader", feature = "writer"))]
pub mod cluster_live;
#[cfg(feature = "writer")]
pub mod cluster_owner;
#[cfg(feature = "writer")]
pub mod cluster_route;
#[cfg(all(feature = "reader", feature = "writer"))]
pub mod commit_push;
#[cfg(all(feature = "writer", feature = "coordinator"))]
pub(crate) mod core_settings;
#[cfg(all(feature = "reader", feature = "writer"))]
mod cost_header;
#[cfg(all(feature = "writer", feature = "coordinator"))]
pub(crate) mod create_settings;
#[cfg(all(feature = "writer", feature = "coordinator"))]
pub mod data_stream_route;
#[cfg(feature = "writer")]
pub mod descriptor_routes;
#[cfg(feature = "writer")]
pub mod doc_route;
#[cfg(feature = "extensions")]
pub mod extension_host;
#[cfg(all(feature = "extensions", feature = "writer"))]
pub mod extension_modules;
#[cfg(all(feature = "extensions", feature = "writer", feature = "coordinator"))]
pub mod extension_pins;
#[cfg(all(feature = "reader", feature = "writer"))]
pub mod extension_refusals;
#[cfg(all(feature = "extensions", feature = "writer"))]
pub mod extension_route;
#[cfg(feature = "extensions")]
pub mod extension_setting;
#[cfg(all(feature = "extensions", feature = "writer"))]
pub mod extension_store;
#[cfg(feature = "writer")]
pub mod forward;
#[cfg(feature = "writer")]
mod forward_call;
pub mod geospatial_setting;
#[cfg(feature = "writer")]
pub mod get_route;
#[cfg(any(feature = "reader", feature = "writer", feature = "coordinator"))]
pub mod handlers;
#[cfg(feature = "writer")]
pub mod health_route;
#[cfg(feature = "writer")]
pub mod id_lanes;
#[cfg(all(feature = "reader", feature = "writer", feature = "coordinator"))]
mod index_assemble;
#[cfg(feature = "writer")]
pub mod index_cache;
#[cfg(all(feature = "writer", feature = "coordinator"))]
pub mod index_delete;
#[cfg(all(feature = "writer", feature = "coordinator"))]
pub mod index_route;
#[cfg(all(feature = "writer", feature = "coordinator"))]
pub mod index_templates;
#[cfg(feature = "writer")]
pub mod ingest_run;
#[cfg(all(feature = "reader", feature = "writer"))]
pub mod ism_route;
#[cfg(all(feature = "reader", feature = "writer"))]
pub mod ism_run;
pub mod ism_setting;
#[cfg(feature = "reader")]
pub mod jobs;
#[cfg(all(feature = "reader", feature = "writer"))]
pub mod knn_model_route;
#[cfg(all(feature = "writer", feature = "coordinator"))]
pub mod knn_setting;
#[cfg(all(feature = "reader", feature = "writer"))]
pub(crate) mod knn_train_request;
#[cfg(all(feature = "reader", feature = "writer"))]
pub mod knn_train_route;
#[cfg(all(feature = "reader", feature = "writer"))]
pub mod lease_age;
#[cfg(all(feature = "reader", feature = "writer"))]
pub mod lease_announce;
#[cfg(feature = "writer")]
pub mod lifecycle;
pub mod lifecycle_bus;
#[cfg(all(feature = "reader", feature = "writer"))]
pub mod ltr_route;
pub mod ltr_setting;
#[cfg(all(feature = "writer", feature = "coordinator"))]
pub(crate) mod metadata_block;
#[cfg(all(feature = "reader", feature = "writer"))]
pub mod ml_route;
pub mod ml_setting;
#[cfg(all(feature = "writer", feature = "coordinator"))]
pub mod nested_setting;
#[cfg(feature = "writer")]
pub mod newest;
#[cfg(feature = "reader")]
pub mod newest_ask;
#[cfg(all(feature = "reader", feature = "writer"))]
pub mod notifications_assemble;
#[cfg(all(feature = "reader", feature = "writer"))]
pub mod notifications_route;
pub mod notifications_setting;
#[cfg(all(feature = "reader", feature = "writer"))]
pub mod observability_route;
pub mod observability_setting;
#[cfg(feature = "reader")]
pub mod open_indexes;
#[cfg(feature = "reader")]
pub mod overlay_builder;
#[cfg(feature = "reader")]
mod overlay_parts;
#[cfg(feature = "writer")]
pub mod pack_holds;
#[cfg(feature = "writer")]
pub mod pack_pass;
#[cfg(all(feature = "reader", feature = "writer"))]
mod page_cache_disk;
#[cfg(feature = "reader")]
pub mod painless_setting;
#[cfg(all(feature = "writer", feature = "coordinator"))]
pub mod pin_policy;
#[cfg(feature = "writer")]
pub mod pipeline_route;
#[cfg(feature = "writer")]
pub mod pit_route;
#[cfg(feature = "writer")]
pub mod plugin_proxy;
#[cfg(feature = "reader")]
pub mod plugins;
#[cfg(feature = "reader")]
pub mod push;
#[cfg(feature = "writer")]
pub mod push_link;
#[cfg(feature = "writer")]
pub mod push_sender;
#[cfg(feature = "writer")]
pub mod read_forward;
pub mod reader_ramp_setting;
#[cfg(feature = "writer")]
pub mod refresh_route;
#[cfg(all(feature = "reader", feature = "writer"))]
pub mod reports_route;
pub mod reports_setting;
#[cfg(feature = "writer")]
pub mod restore_background;
#[cfg(all(feature = "writer", feature = "coordinator"))]
pub(crate) mod restore_settings;
#[cfg(all(feature = "writer", feature = "coordinator"))]
pub mod rollover_route;
#[cfg(feature = "writer")]
pub(crate) mod route_util;
#[cfg(feature = "writer")]
pub mod script_route;
#[cfg(feature = "reader")]
pub mod search;
#[cfg(feature = "reader")]
pub mod search_modules;
#[cfg(all(feature = "reader", feature = "writer"))]
pub mod search_pipeline_route;
#[cfg(all(feature = "reader", feature = "writer"))]
pub mod search_queue;
#[cfg(all(feature = "reader", feature = "writer"))]
pub mod search_shard_action;
#[cfg(feature = "reader")]
pub mod search_slots;
#[cfg(feature = "writer")]
pub mod security_auth;
#[cfg(feature = "writer")]
#[cfg(feature = "writer")]
pub mod security_authz;
pub(crate) mod security_authz_all_access;
#[cfg(feature = "writer")]
pub mod security_route;
pub mod security_setting;
#[cfg(feature = "writer")]
pub mod security_state;
pub mod settings;
#[cfg(all(feature = "writer", feature = "coordinator"))]
pub mod settings_route;
#[cfg(feature = "writer")]
pub(crate) mod snapshot_global_capture;
#[cfg(all(feature = "writer", feature = "coordinator"))]
mod snapshot_restore_aliases;
#[cfg(all(feature = "writer", feature = "coordinator"))]
mod snapshot_restore_background;
#[cfg(all(feature = "writer", feature = "coordinator"))]
mod snapshot_restore_global;
#[cfg(all(feature = "writer", feature = "coordinator"))]
pub mod snapshot_restore_route;
#[cfg(feature = "writer")]
pub mod snapshot_route;
#[cfg(feature = "writer")]
pub mod snapshot_take_route;
#[cfg(all(feature = "reader", feature = "writer"))]
pub mod sql_route;
pub mod sql_setting;
#[cfg(feature = "writer")]
pub mod stock_export_route;
#[cfg(all(feature = "writer", feature = "coordinator"))]
pub mod stock_restore_route;
#[cfg(feature = "writer")]
pub mod stock_s3;
#[cfg(any(feature = "reader", feature = "writer"))]
pub mod stored_script_source;
#[cfg(all(feature = "reader", feature = "writer"))]
pub mod stored_template_source;
#[cfg(feature = "writer")]
pub mod tail_await;
#[cfg(feature = "reader")]
pub mod tail_fresh;
#[cfg(feature = "reader")]
pub mod tail_held;
#[cfg(feature = "writer")]
pub mod tail_keep;
#[cfg(feature = "writer")]
pub mod tail_link;
#[cfg(all(feature = "reader", feature = "writer"))]
pub mod tail_node;
#[cfg(feature = "reader")]
pub mod tail_overlay;
#[cfg(feature = "writer")]
pub mod tail_push;
#[cfg(feature = "reader")]
pub mod tail_reader_link;
#[cfg(feature = "writer")]
pub mod tail_serve;
#[cfg(feature = "writer")]
mod tail_subscription;
#[cfg(feature = "writer")]
pub mod template_route;
#[cfg(feature = "writer")]
pub mod tenants;
#[cfg(any(feature = "reader", feature = "writer"))]
pub(crate) mod time_value;
#[cfg(feature = "writer")]
pub mod update_body;
#[cfg(feature = "writer")]
pub mod update_bulk;
#[cfg(feature = "writer")]
pub mod update_decide;
#[cfg(feature = "writer")]
pub mod update_route;
#[cfg(all(feature = "extensions", feature = "reader", feature = "writer"))]
mod usage_extensions;
#[cfg(all(feature = "reader", feature = "writer"))]
pub mod usage_metrics;
#[cfg(all(feature = "reader", feature = "writer"))]
pub mod usage_route;
pub mod vector_warm_setting;
#[cfg(all(feature = "reader", feature = "writer"))]
pub mod warm_boot;
#[cfg(all(feature = "reader", feature = "writer"))]
pub mod warm_copy;
#[cfg(all(feature = "reader", feature = "writer"))]
pub mod warm_expect;
#[cfg(all(feature = "reader", feature = "writer"))]
pub mod warm_handlers;
#[cfg(all(feature = "reader", feature = "writer"))]
pub mod warm_join;
#[cfg(all(feature = "reader", feature = "writer"))]
pub mod warm_peer;
#[cfg(feature = "reader")]
pub mod warm_pressure;
#[cfg(all(feature = "reader", feature = "writer"))]
pub mod warm_rate;
#[cfg(all(feature = "reader", feature = "writer"))]
pub mod warm_setting;
#[cfg(feature = "writer")]
pub mod writer;

/// A role a node can serve.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    /// Serves searches from published segments (`reader` feature).
    Reader,
    /// Makes writes durable and publishes segments (`writer` feature).
    Writer,
    /// Routes, fans out and reduces requests (`coordinator` feature).
    Coordinator,
}

impl Role {
    /// The role's name, as used in configuration and logs.
    pub const fn name(self) -> &'static str {
        match self {
            Role::Reader => "reader",
            Role::Writer => "writer",
            Role::Coordinator => "coordinator",
        }
    }
}

/// The roles this binary was built with, in a fixed order.
pub fn enabled_roles() -> Vec<Role> {
    let mut roles = Vec::with_capacity(3);
    if cfg!(feature = "reader") {
        roles.push(Role::Reader);
    }
    if cfg!(feature = "writer") {
        roles.push(Role::Writer);
    }
    if cfg!(feature = "coordinator") {
        roles.push(Role::Coordinator);
    }
    roles
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roles_follow_the_build_switches() {
        let roles = enabled_roles();
        assert_eq!(roles.contains(&Role::Reader), cfg!(feature = "reader"));
        assert_eq!(roles.contains(&Role::Writer), cfg!(feature = "writer"));
        assert_eq!(
            roles.contains(&Role::Coordinator),
            cfg!(feature = "coordinator")
        );
    }

    #[test]
    fn names_are_distinct() {
        let names = [Role::Reader, Role::Writer, Role::Coordinator].map(Role::name);
        assert_eq!(names, ["reader", "writer", "coordinator"]);
    }
}
