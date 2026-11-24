//! Which index a blob key belongs to, read from the key alone.
//!
//! The keys that name an index are the descriptor register
//! (`indices/<index>`), a tombstone marker (`tombstones/<hour>/<index>`), a
//! reclaim intent (`reclaim/<minute>/<index>#<uuid>`), a shard's storage
//! (`segments/<index>#<uuid>#<shard>/...`) and a shard's head
//! (`shards/<2 hex digits>-<index>#<shard>`). Every other key (node packs,
//! node logs, leases, members, change stripes, cluster settings, points in
//! time) belongs to no one index: a node pack and a node log carry many.

/// The index `key` names, under `base` (empty, or ending in `/`); `None` for
/// a key outside `base` or one no single index owns.
pub fn index_of_key<'a>(base: &str, key: &'a str) -> Option<&'a str> {
    let rest = key.strip_prefix(base)?;
    let (kind, tail) = rest.split_once('/')?;
    let name = match kind {
        "indices" => tail,
        "tombstones" | "reclaim" => tail.split_once('/')?.1,
        "segments" => tail.split_once('/')?.0,
        "shards" => tail.split_once('-')?.1,
        _ => return None,
    };
    let index = match kind {
        "indices" | "tombstones" => name,
        _ => name.split('#').next()?,
    };
    (!index.is_empty() && !index.contains('/')).then_some(index)
}
