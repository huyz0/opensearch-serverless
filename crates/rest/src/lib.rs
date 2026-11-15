//! The HTTP edge: server, streaming bodies and routing.
//!
//! Request bodies are parsed in bounded chunks from pooled buffers and
//! response bodies are written the same way (ADR 0004). Search semantics live
//! in the role crates; this crate knows HTTP.
