//! OpenSearch's native transport, the wire the Java shell's nodes forward
//! writes, searches, pushes and cache warm-ups over, ported so Rust and Java
//! nodes talk to each other directly (ADR 0016).
//!
//! This part holds the encodings: OpenSearch's stream primitives, the
//! frame, the two handshakes and `DiscoveryNode`, each tested against bytes
//! the Java's own classes wrote. The client, server and forwarded actions
//! build on them.

pub mod channel;
pub mod client;
pub mod error;
pub mod exception;
pub mod frame;
pub mod handshake;
pub mod node;
pub mod server;
pub mod stream;
pub mod version;

pub use error::WireError;
