//! The node binary's composition root.
//!
//! Each role is compiled in only when its build switch (a Cargo feature) is
//! on, so a reader-only build carries no writer code. [`enabled_roles`] reports
//! what this binary was built with.

#[cfg(all(feature = "reader", feature = "writer"))]
mod admin_assemble;
#[cfg(all(feature = "reader", feature = "writer"))]
pub mod assemble;
#[cfg(all(feature = "reader", feature = "writer"))]
mod assemble_error;
#[cfg(all(feature = "reader", feature = "writer"))]
pub mod boot;
#[cfg(feature = "writer")]
pub mod bulk_handler;
#[cfg(feature = "writer")]
pub mod bulk_route;
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
#[cfg(feature = "writer")]
pub mod descriptor_routes;
#[cfg(feature = "writer")]
pub mod doc_route;
#[cfg(feature = "writer")]
pub mod forward;
#[cfg(feature = "writer")]
mod forward_call;
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
#[cfg(all(feature = "writer", feature = "coordinator"))]
pub mod knn_setting;
#[cfg(all(feature = "reader", feature = "writer"))]
pub mod lease_announce;
#[cfg(feature = "writer")]
pub mod lifecycle;
#[cfg(feature = "writer")]
pub mod newest;
#[cfg(feature = "reader")]
pub mod newest_ask;
#[cfg(feature = "reader")]
pub mod open_indexes;
#[cfg(feature = "reader")]
pub mod overlay_builder;
#[cfg(all(feature = "reader", feature = "writer"))]
mod page_cache_disk;
#[cfg(feature = "writer")]
pub mod pipeline_route;
#[cfg(feature = "writer")]
pub mod pit_route;
#[cfg(feature = "writer")]
pub mod plugin_proxy;
#[cfg(feature = "reader")]
pub mod push;
#[cfg(feature = "writer")]
pub mod push_link;
#[cfg(feature = "writer")]
pub mod push_sender;
#[cfg(feature = "writer")]
pub mod read_forward;
#[cfg(feature = "writer")]
pub mod refresh_route;
#[cfg(feature = "writer")]
pub(crate) mod route_util;
#[cfg(feature = "reader")]
pub mod search;
#[cfg(feature = "reader")]
pub mod search_slots;
#[cfg(feature = "writer")]
pub mod security_auth;
#[cfg(feature = "writer")]
pub mod security_route;
pub mod security_setting;
#[cfg(feature = "writer")]
pub mod security_state;
pub mod settings;
#[cfg(feature = "writer")]
pub mod snapshot_route;
#[cfg(feature = "writer")]
pub mod snapshot_take_route;
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
#[cfg(all(feature = "reader", feature = "writer"))]
pub mod usage_route;
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
