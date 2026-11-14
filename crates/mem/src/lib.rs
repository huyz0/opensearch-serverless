//! The node memory budget and pooled buffers.
//!
//! Every node has a fixed memory budget split into named pools. A component
//! reserves from its pool before it allocates and releases on drop; when a
//! pool is exhausted the caller answers 429 or applies backpressure. I/O uses
//! fixed-size buffers from a per-node pool. See
//! `docs/adr/0004-bounded-memory-and-streaming.md`.
