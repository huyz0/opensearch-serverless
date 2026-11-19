//! Where a get is served, and whether the version a caller pinned holds.

use crate::route::place::{PlaceError, Placement, Plane, Refusal, Router};

/// `Versions.MATCH_ANY`: no version is pinned.
pub const MATCH_ANY: i64 = -3;
/// `Versions.MATCH_DELETED`: the document must have been deleted.
pub const MATCH_DELETED: i64 = -4;
/// `Versions.NOT_FOUND`: the version of a document that does not exist.
pub const NOT_FOUND: i64 = -1;

/// Where one get is served.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GetPlan {
    /// Read from the writer this node holds.
    Local,
    /// The document has never existed: nothing was ever published and no
    /// node owns the shard.
    Absent,
    /// Read from the published commit: nobody owns the shard, so no
    /// unpublished writes exist.
    Reader,
    /// Ask the node that owns the shard.
    Forward {
        /// The owner's node id.
        owner: String,
    },
}

/// Where the get at `placement` is served, from `local_node`.
///
/// `published` says whether the shard has ever published a commit; it is
/// called only when no node owns the shard.
///
/// # Errors
///
/// [`Refusal::Metadata`] when `published` fails, and the router's
/// [`Router::not_here`] refusal when this node owns the shard without
/// holding it.
pub fn plan_get<P: Plane>(
    router: &Router<'_, P>,
    index: &str,
    placement: &Placement,
    local_node: &str,
    published: impl FnOnce() -> Result<bool, PlaceError>,
) -> Result<GetPlan, Refusal> {
    let owner = placement.owner.as_deref();
    // A writer held after the head moved has not noticed it lost the shard,
    // and lags the owner's acknowledged writes: only a head that agrees, or
    // names nobody, lets the local one answer.
    if placement.local && owner.is_none_or(|o| o == local_node) {
        return Ok(GetPlan::Local);
    }
    match owner {
        None => {
            if published().map_err(|e| Refusal::Metadata(e.0))? {
                Ok(GetPlan::Reader)
            } else {
                Ok(GetPlan::Absent)
            }
        }
        Some(o) if o == local_node => Err(router.not_here(index, placement)),
        Some(o) => Ok(GetPlan::Forward {
            // ALLOC: one owner id.
            owner: o.to_owned(),
        }),
    }
}

/// The words of a version conflict between a document at `current` and a
/// get pinned to `expected`, or `None` when the pin holds.
#[must_use]
pub fn version_conflict(current: i64, expected: i64) -> Option<String> {
    if expected == MATCH_ANY {
        return None;
    }
    if current == NOT_FOUND {
        // ALLOC: one message.
        return Some(format!(
            "document does not exist (expected version [{expected}])"
        ));
    }
    if expected == MATCH_DELETED || current != expected {
        // ALLOC: one message.
        return Some(format!(
            "current version [{current}] is different than the one provided [{expected}]"
        ));
    }
    None
}
