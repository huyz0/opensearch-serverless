//! A shard container's name, `<index>#<uuid>#<shard>`, judged only if this
//! system wrote it.

use oss_format::{MAX_SHARDS, looks_like_an_id};
use oss_meta::validate_index_or_alias;

/// The parts of a container name this system writes.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Container<'a> {
    pub index: &'a str,
    pub uuid: &'a str,
}

/// The container `name` names, or `None` when it is not one this system
/// wrote: an index that fails the name rules, a uuid that is not 22 URL-safe
/// characters, or a shard that is not a plain integer in `0..MAX_SHARDS`. A
/// name that is left is never deleted.
pub(super) fn parse(name: &str) -> Option<Container<'_>> {
    let mut parts = name.split('#');
    let (index, uuid, shard) = (parts.next()?, parts.next()?, parts.next()?);
    if parts.next().is_some() {
        return None;
    }
    validate_index_or_alias(index).ok()?;
    if !looks_like_an_id(uuid) {
        return None;
    }
    let number: u32 = shard.parse().ok()?;
    // Plain digits only: no sign, no leading zero.
    if number >= MAX_SHARDS || shard.bytes().any(|b| !b.is_ascii_digit()) {
        return None;
    }
    if shard.len() > 1 && shard.starts_with('0') {
        return None;
    }
    Some(Container { index, uuid })
}
