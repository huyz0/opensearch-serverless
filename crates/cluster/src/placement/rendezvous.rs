//! Rendezvous (highest random weight) hashing: every node computes the same
//! owner for a key from the same members, and a member leaving moves only
//! its own keys. Ported from `cluster/Rendezvous.java` and
//! `cluster/ReaderPlacement.java`.

use oss_format::{NodeLease, java_order};

use super::hash::{ShardKey, decimal, murmur3};

/// The weight of `node_id` for `key` joined by `sep` (`/` for a writer,
/// `#` for a reader), as the Java's `Murmur3HashFunction.hash(nodeId + "/" + key)`.
fn weight(node_id: &str, key: ShardKey<'_>, sep: &str) -> i32 {
    let mut digits = [0u8; 11];
    let shard = decimal(key.shard, &mut digits);
    murmur3(&[node_id, "/", key.index, sep, shard])
}

/// Whether `(a, a_id)` ranks before `(b, b_id)`: the higher weight, then
/// the lower id in Java's order.
fn before(a: i32, a_id: &str, b: i32, b_id: &str) -> bool {
    a > b || (a == b && java_order(a_id, b_id).is_lt())
}

/// The node of `node_ids` that owns `key`'s writes (`Rendezvous.owner` of
/// `index/shard`), or `None` for no candidates.
pub fn owner<'a>(
    key: ShardKey<'_>,
    node_ids: impl IntoIterator<Item = &'a str>,
) -> Option<&'a str> {
    let mut best: Option<(&'a str, i32)> = None;
    for id in node_ids {
        let w = weight(id, key, "/");
        if best.is_none_or(|(b, bw)| before(w, id, bw, b)) {
            best = Some((id, w));
        }
    }
    best.map(|(id, _)| id)
}

/// The `limit` members with `role` that rank highest for reading `key`
/// (`ReaderPlacement.candidatesFor`), best first. Each member is hashed
/// once and only `limit` are kept, so a search does not sort the fleet.
pub fn readers_for<'a>(
    key: ShardKey<'_>,
    members: impl IntoIterator<Item = &'a NodeLease>,
    role: &str,
    limit: usize,
) -> Vec<&'a str> {
    if limit == 0 {
        // ALLOC: none.
        return Vec::new();
    }
    // ALLOC: at most `limit` entries; the common two-reader route keeps two.
    let mut best: Vec<(i32, &'a str)> = Vec::with_capacity(limit.min(16));
    for lease in members {
        if !super::has_role(lease, role) {
            continue;
        }
        let id = lease.node_id.as_str();
        let w = weight(id, key, "#");
        let at = best.partition_point(|(bw, b)| before(*bw, b, w, id));
        if at < limit {
            if best.len() == limit {
                best.pop();
            }
            best.insert(at, (w, id));
        }
    }
    // ALLOC: at most `limit` ids.
    best.into_iter().map(|(_, id)| id).collect()
}

/// The `limit` live ingest writers that reap the dead node `dead_id`, best
/// first: the members that rank highest by rendezvous on the dead node's id.
/// Every node computes the same answer from the same members; capacity and
/// load are ignored, and a member that is revoked or is `dead_id` is not a
/// candidate.
pub fn reapers_for<'a>(
    dead_id: &str,
    members: impl IntoIterator<Item = &'a NodeLease>,
    limit: usize,
) -> Vec<&'a str> {
    if limit == 0 {
        // ALLOC: none.
        return Vec::new();
    }
    // ALLOC: at most `limit` entries, the reaper count (two by default).
    let mut best: Vec<(i32, &'a str)> = Vec::with_capacity(limit.min(16));
    for lease in members {
        let id = lease.node_id.as_str();
        if lease.revoked || id == dead_id || !super::has_role(lease, super::ROLE_INGEST) {
            continue;
        }
        let w = murmur3(&["reap", "/", dead_id, "/", id]);
        let at = best.partition_point(|(bw, b)| before(*bw, b, w, id));
        if at < limit {
            if best.len() == limit {
                best.pop();
            }
            best.insert(at, (w, id));
        }
    }
    // ALLOC: at most `limit` ids.
    best.into_iter().map(|(_, id)| id).collect()
}
