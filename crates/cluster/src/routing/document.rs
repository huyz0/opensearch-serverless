//! Which shard a document goes to (`ops/DocumentRouting`): through the unit
//! layout for an index created with a shard mode, and through core's
//! `OperationRouting.generateShardId` otherwise.

use std::collections::BTreeSet;

use crate::placement::murmur3;

use super::RoutingError;
use super::layout::UnitLayout;

/// How an index routes documents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocumentRouting<'a> {
    /// An index without a shard mode: core's murmur3 (32-bit, over UTF-16)
    /// modulo the shard count, as the shell renders it (routing shards
    /// equal to the shard count, so the routing factor is 1). An index
    /// with `index.number_of_virtual_shards` is not routed here.
    Legacy {
        /// `number_of_shards`.
        shards: u32,
        /// `index.routing_partition_size`, 1 when unset.
        partition_size: u32,
    },
    /// An index with a shard mode: the 64-bit hash in the unit layout. The
    /// answer is a unit id, which is the physical shard.
    Units(&'a UnitLayout),
}

impl<'a> DocumentRouting<'a> {
    /// Routing for an index without a shard mode.
    ///
    /// # Errors
    ///
    /// [`RoutingError::Invalid`] when `shards` is zero, or `partition_size`
    /// is zero or, past 1, not below `shards` (core refuses such an index).
    pub fn legacy(shards: u32, partition_size: u32) -> Result<Self, RoutingError> {
        if shards < 1 || partition_size < 1 || (partition_size != 1 && partition_size >= shards) {
            return Err(RoutingError::Invalid(
                "a legacy index needs shards and a partition size below them",
            ));
        }
        Ok(Self::Legacy {
            shards,
            partition_size,
        })
    }

    /// The shard a document routes to: by `routing` when given, by `id`
    /// otherwise. A routing partition spreads one routing value over
    /// `partition_size` shards, chosen by the id.
    ///
    /// # Errors
    ///
    /// [`RoutingError::RoutingMissing`] for a partitioned index and no
    /// routing value, which core requires there.
    pub fn shard_for(&self, id: &str, routing: Option<&str>) -> Result<u32, RoutingError> {
        let effective = routing.unwrap_or(id);
        match *self {
            Self::Units(layout) => Ok(layout.unit_for_routing(effective).id),
            Self::Legacy {
                shards,
                partition_size,
            } => {
                let offset = if partition_size == 1 {
                    0
                } else if routing.is_none() {
                    return Err(RoutingError::RoutingMissing);
                } else {
                    floor_mod(murmur3(&[id]), partition_size)
                };
                // Java's int addition wraps; the offset is below 2^31.
                let hash = murmur3(&[effective]).wrapping_add(i32::try_from(offset).unwrap_or(0));
                Ok(floor_mod(hash, shards))
            }
        }
    }

    /// The shards a search with routing values visits, ascending: the
    /// shard each value routes to. `None` when every shard has to be
    /// searched, which is a legacy index with a routing partition.
    pub fn shards_for_search(&self, routings: &[&str]) -> Option<BTreeSet<u32>> {
        if let Self::Legacy { partition_size, .. } = *self
            && partition_size != 1
        {
            return None;
        }
        routings
            .iter()
            .map(|routing| self.shard_for(routing, Some(routing)).ok())
            // ALLOC: one shard per routing value given.
            .collect()
    }
}

/// Java's `Math.floorMod(hash, n)` for a positive `n`.
fn floor_mod(hash: i32, n: u32) -> u32 {
    let m = i64::from(hash).rem_euclid(i64::from(n.max(1)));
    // In [0, n), so it fits.
    u32::try_from(m).unwrap_or(0)
}

/// A routing parameter as core's requests read it: empty means none.
pub fn normalize(routing: Option<&str>) -> Option<&str> {
    routing.filter(|r| !r.is_empty())
}

/// A search's comma-separated routing parameter split into its distinct
/// non-empty values, in order, as core splits it.
pub fn search_routings(routing: Option<&str>) -> Vec<&str> {
    // ALLOC: one slice per value, bounded by the parameter's length.
    let mut values: Vec<&str> = Vec::new();
    for value in routing.unwrap_or_default().split(',') {
        if !value.is_empty() && !values.contains(&value) {
            values.push(value);
        }
    }
    values
}
