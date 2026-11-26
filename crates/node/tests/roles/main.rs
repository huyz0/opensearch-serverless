//! The writer-role and node-boot tests, built as one test binary: each file is a module, and every test still runs
//! as its own process under nextest. One binary links once, where twenty linked twenty times.

mod boot_tombstone;
mod bucket_orphans;
mod bucket_wiring;
mod bulk_handler;
mod cluster_live;
mod cluster_owner;
mod descriptor_routes;
mod exit_gate;
mod forward;
mod index_cache;
mod orphan_timer;
mod publish_age;
mod publish_read;
mod tombstone_pass;
mod write_gate;
mod writer_first;
mod writer_heads;
mod writer_janitor;
mod writer_logged;
mod writer_newest;
mod writer_opener;
mod writer_opener_pins;
mod writer_opener_rig;
mod writer_publish;
mod writer_publish_timer;
mod writer_role;
mod writer_staging;
mod writer_takeover;
