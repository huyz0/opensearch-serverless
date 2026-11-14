//! The search role: serves queries from immutable published segment blobs.
//!
//! A reader holds no durable state. It learns of new commits from the writer
//! that published them, opens each segment blob by ranged reads through the
//! page cache, and searches tail records the writer pushed to it so fresh
//! writes are visible before they are published. It never writes to the
//! object store. This is the first role built (docs/roadmap.md).
