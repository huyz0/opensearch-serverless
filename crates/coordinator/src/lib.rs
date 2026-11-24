//! The coordinating role: REST, routing, fan-out and reduce.
//!
//! A coordinator parses the request, decides which shards and which nodes
//! hold them, fans the request out, and merges the shard results into one
//! response. It is the last role built (docs/roadmap.md).

pub mod cat;
pub mod cluster;
pub mod docs;
pub mod fanout;
pub mod indices;
pub mod ingest;
pub mod pipeline_admin;
pub mod reduce;
pub mod repository_admin;
pub mod route;
pub mod search;
pub mod search_nested;
pub mod security;
pub mod snapshot_admin;
pub mod snapshot_restore_admin;
pub mod snapshot_take_admin;
pub mod template_admin;
pub mod templates;
pub mod zone_meter;
