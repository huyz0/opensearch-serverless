//! The coordinating role: REST, routing, fan-out and reduce.
//!
//! A coordinator parses the request, decides which shards and which nodes
//! hold them, fans the request out, and merges the shard results into one
//! response. It is the last role built (docs/roadmap.md).

pub mod cluster;
pub mod docs;
pub mod fanout;
pub mod indices;
pub mod reduce;
pub mod route;
pub mod search;
pub mod zone_meter;
