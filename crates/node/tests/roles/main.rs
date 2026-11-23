//! The writer-role and node-boot tests, built as one test binary: each file is a module, and every test still runs
//! as its own process under nextest. One binary links once, where twenty linked twenty times.

mod publish_age;
mod publish_read;
mod writer_first;
mod writer_heads;
mod writer_janitor;
mod writer_newest;
mod writer_opener;
mod writer_publish;
mod writer_publish_timer;
mod writer_role;
mod writer_takeover;
mod forward;
mod descriptor_routes;
mod index_cache;
mod exit_gate;
mod boot_tombstone;
mod bulk_handler;
mod cluster_owner;
mod cluster_live;
mod tombstone_pass;
