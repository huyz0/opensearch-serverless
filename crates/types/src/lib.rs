//! Identifiers and value types shared by every crate.
//!
//! This crate depends on nothing else in the workspace. Distinct identifiers
//! (tenant, index, shard, node) are distinct types, so passing one where
//! another belongs does not compile. They are added here as the reader, writer
//! and coordinator need them.
//!
//! [`settings`] defines settings: each module declares its own, with the
//! Java shell's keys, types, defaults and messages, and the node collects
//! them into one registry.
//!
//! [`stored_scripts`] is the resolver a node installs for a script a request
//! names by `id`, shared by the roles that parse a script.

pub mod settings;
pub mod stored_scripts;
