//! A source and a mapping as the Lucene document OpenSearch indexes for it.
//!
//! The writer indexes with it and the reader builds its tail overlay with it, so
//! the two cannot disagree about what a source becomes ([ADR 0019]).
//!
//! [ADR 0019]: https://github.com/huyz0/opensearch-serverless/blob/main/docs/adr/0019-document-indexing-crate-below-reader-and-writer.md

pub mod document;
pub mod fast;
pub mod lucene;
pub mod mapping;
