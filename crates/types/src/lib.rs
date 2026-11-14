//! Identifiers and value types shared by every crate.
//!
//! This crate depends on nothing else in the workspace. Distinct identifiers
//! (tenant, index, shard, node) are distinct types, so passing one where
//! another belongs does not compile. They are added here as the reader, writer
//! and coordinator need them.
