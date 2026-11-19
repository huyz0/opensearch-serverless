//! Where a write is applied, and which native write options are accepted.

use crate::route::place::{Placement, Plane, Refusal, Router};

/// Where one write goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WritePlan {
    /// Apply it on the writer this node holds.
    Local,
    /// Ask this node to take the shard, then place the write again.
    Activate,
    /// Ask the node that owns the shard.
    Forward {
        /// The owner's node id.
        owner: String,
    },
}

/// How a request's version is judged: only the internal kind is accepted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VersionKind {
    /// `VersionType.INTERNAL`.
    Internal,
    /// `VersionType.EXTERNAL`.
    External,
    /// `VersionType.EXTERNAL_GTE`.
    ExternalGte,
}

/// `wait_for_active_shards`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActiveShards {
    /// The index's default.
    Default,
    /// Every shard copy.
    All,
    /// This many copies.
    Count(i32),
}

const MATCH_ANY: i64 = -3;
const MATCH_DELETED: i64 = -4;

/// Where the write at `placement` goes, from `local_node`; `activated` once
/// the node has been asked to take the shard and the write placed again.
///
/// # Errors
///
/// The router's [`Router::not_here`] refusal when nobody, or this node,
/// owns a shard this node does not hold and activating it did not help.
pub fn plan_write<P: Plane>(
    router: &Router<'_, P>,
    index: &str,
    placement: &Placement,
    local_node: &str,
    activated: bool,
) -> Result<WritePlan, Refusal> {
    if placement.local {
        return Ok(WritePlan::Local);
    }
    match placement.owner.as_deref() {
        Some(owner) if owner != local_node => Ok(WritePlan::Forward {
            // ALLOC: one owner id.
            owner: owner.to_owned(),
        }),
        _ if !activated => Ok(WritePlan::Activate),
        _ => Err(router.not_here(index, placement)),
    }
}

/// The placement to use after an activation: `again` only for the index
/// incarnation the write was routed for, since an index recreated meanwhile
/// is a new routing decision.
#[must_use]
pub fn settle(before: &Placement, again: Placement) -> Placement {
    if again.uuid == before.uuid {
        again
    } else {
        before.clone()
    }
}

/// The version and active-shard options the native client may send.
///
/// # Errors
///
/// The shell's words for an option it cannot honour: versions other than
/// none (or "deleted" on a create) and the other version kinds, and
/// `wait_for_active_shards` above one.
pub fn validate_native_write(
    version: i64,
    kind: VersionKind,
    active: &ActiveShards,
    create: bool,
) -> Result<(), &'static str> {
    let version_ok = kind == VersionKind::Internal
        && (version == MATCH_ANY || (create && version == MATCH_DELETED));
    if !version_ok {
        return Err("version/version_type is not supported; use if_seq_no and if_primary_term");
    }
    // Shards have no replicas: one copy is every copy there is.
    if matches!(active, ActiveShards::Count(n) if *n > 1) {
        return Err("wait_for_active_shards cannot exceed one: shards have no replicas");
    }
    Ok(())
}
