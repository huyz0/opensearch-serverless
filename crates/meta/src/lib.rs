//! The metadata plane on registers.
//!
//! What names exist and what they hold lives in the object store, one
//! register a name, and changes only by compare-and-swap, as in the Java
//! shell's `metadata` package: no cluster manager, no lock and no leader.
//! Rust and Java nodes on one bucket see the same names.
//!
//! - [`validate_index_or_alias`]: what a name may be before it becomes a key.
//! - [`DescriptorStore`]: create, read, update and delete an index's
//!   descriptor register, with tombstones keeping a deleted name's
//!   generations moving.
//! - [`DescriptorStore::create_alias`], [`DescriptorStore::update_alias`]
//!   and [`DescriptorStore::resolve`]: aliases in the same namespace, so a
//!   name is an index or an alias and resolving it is one read.
//! - [`AliasMutations`]: creating, changing and deleting an alias so every
//!   member's `aliased_by` hint names it before it is published, and a
//!   crashed writer's half-done change is recovered.
//! - [`DeleteMode::probe`]: whether a store's conditional delete can be
//!   trusted, asked of the store itself.
//! - [`DescriptorStore::sweep_tombstones`]: removing tombstones once their
//!   quarantine has passed, found from their hourly markers.
//! - [`ChangeStripes`]: a few registers every descriptor update and delete
//!   bumps, so a writer confirms many indexes unchanged with one read.
//! - [`DescriptorStore::list_page`] and [`DescriptorStore::names_with_prefix`]:
//!   the one enumeration of names, a bounded page at a time, and what an
//!   index pattern matches, at a cost set by its cap.
//! - [`RoutingCache`]: what routing may answer from a read up to a second
//!   old, forgotten on this node's own changes through
//!   [`DescriptorStore::on_changed`].
//! - [`ClusterConfig`]: an operator's persistent settings, one register a
//!   deployment, read and updated as the Java's `ClusterConfig`.
//! - [`PointInTimeStore`]: frozen views of an index, each a state register
//!   and an inventory marker that keeps its files, released, extended and
//!   expired by compare-and-swap.
//! - [`DocSet`]: index and component templates and ingest and search
//!   pipelines, a few documents each, named by one marker register and
//!   changed by one swap of it.
//! - [`DocSet::snapshot`] and [`DocSetCache`]: every document of a set at the
//!   cost of one marker read while it has not moved.
//! - [`IndexLifecycle`]: creating and deleting an index, idempotent on a
//!   retried create, and finishing a delete from the [`ReclaimQueue`] once a
//!   writer that missed it has stopped.

mod alias_mutations;
mod changes;
mod cluster_config;
mod descriptors;
mod doc_cache;
mod doc_set;
mod error;
mod lifecycle;
mod listing;
mod names;
mod pit;
mod probe;
mod ranged_source;
mod repositories;
mod routing;
mod snapshot_capture;
mod snapshot_remove;
mod snapshot_restore;
mod snapshot_take;
mod snapshots;
mod stock_fan;
mod stock_plan;
mod stock_read;
mod stock_restore;
mod sweep;

pub use alias_mutations::{AliasAt, AliasClock, AliasMutations};
pub use changes::{ChangeStripes, Snapshot};
pub use cluster_config::{ClusterConfig, ConfigValue};
pub use descriptors::{
    BUCKET_MILLIS, DEFAULT_QUARANTINE_MILLIS, DeleteMode, DescriptorStore, Listener,
    REAP_DEADLINE_MILLIS, REAP_HANDOFF_MILLIS, Resolution, Target,
};
pub use doc_cache::{DEFAULT_DOC_CACHE_BYTES, DocSetCache, DocSnapshot};
pub use doc_set::DocSet;
pub use error::MetaError;
pub use lifecycle::{
    DEFAULT_RECLAIM_DELAY_MILLIS, Deleted, HeadAt, IndexLifecycle, Purged, ReclaimQueue,
    ShardHeads, ShardPins,
};
pub use names::{MAX_NAME_BYTES, NameError, NameRule, validate_index_or_alias, validate_prefix};
pub use oss_format::{
    AliasMutation, AliasRecord, ChangeKind, ClusterSettings, DocKind, MAX_CLUSTER_SETTINGS_BYTES,
    MAX_DOC_BYTES, MAX_DOC_SET_MARKER_BYTES, MAX_DOCS, PitFile, PitShard, PointInTime, RELEASED,
    ReclaimIntent, STRIPES, Setting, SettingsRefused,
};
pub use pit::{CaptureError, PitClock, PointInTimeStore, new_id};
pub use probe::PROBE_PREFIX;
pub use repositories::{MAX_SNAPSHOT_MARKERS, Removal, RepositoryWriteError, RepositoryWriter};
pub use routing::{
    DEFAULT_ROUTING_CACHE_BYTES, DEFAULT_ROUTING_CACHE_MILLIS, DEFAULT_ROUTING_CACHE_PRUNE_ABOVE,
    RoutingCache,
};
pub use snapshot_capture::DescribedIndex;
pub use snapshot_restore::{CopyBuffers, RestoreError, SnapshotRestorer};
pub use snapshot_take::{Reservation, SnapshotTaker, TakeError};
pub use snapshots::{
    MAX_REPOSITORIES, RestoreRead, SnapshotRead, SnapshotReadError, SnapshotReader,
};
pub use stock_plan::{
    MAX_FILE_ENTRIES, MAX_INDICES, MAX_SHARDS, PlannedIndex, StockLimits, StockPlan,
    StockPlanError, StockRules, load as stock_load, plan as stock_plan,
};
pub use stock_read::{MAX_BLOB_BYTES, MAX_INDEX_N_BYTES, StockReadError, StockRepo};
pub use stock_restore::{StockShardCopy, StockSource};
pub use sweep::{MARKERS_PREFIX, Sweep, Swept};
