//! The HTTP edge: server, streaming bodies and routing.
//!
//! A blocking HTTP/1.1 server on a fixed pool of worker threads
//! ([ADR 0006](../../../docs/adr/0006-http-stack.md)). Every buffer it uses is
//! charged to the node's request-body pool when it starts, and a request
//! with a body takes a body buffer before a byte of the body is read. Bodies
//! stream to the handler in chunks of one buffer, and responses stream back
//! the same way (ADR 0004). [`json::Tokenizer`] and [`ndjson::Splitter`]
//! parse those chunks without holding a whole body. Errors are OpenSearch's
//! error bodies. Search semantics live in the role crates; this crate knows
//! HTTP.

mod body;
mod checks;
mod config;
mod conn;
mod error;
mod head;
mod inbound;
pub mod json;
mod json_out;
mod method;
pub mod ndjson;
mod params;
mod query;
mod request;
mod response;
mod router;
mod server;
mod timed;

pub use body::Body;
pub use config::{ConfigError, ServerConfig};
pub use error::{Exception, Meta, RestError};
pub use head::Version;
pub use json_out::write_str as write_json_str;
pub use method::{Method, MethodSet};
pub use query::decode_component;
pub use request::Request;
pub use response::{BodyWriter, JSON, Length, Response};
pub use router::{Handler, MAX_SEGMENTS, RouteError, Router};
pub use server::{Server, StartError};
