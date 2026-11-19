//! Routing each bulk item to a shard, and grouping items by shard.

use std::collections::HashMap;

use crate::route::place::PlaceError;

/// Where a name's writes land.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    /// The backing index a write lands on (the index itself unless the name
    /// is a data stream).
    pub index: String,
    /// That index's uuid.
    pub uuid: String,
    /// Whether the name is a data stream.
    pub data_stream: bool,
}

/// What routing a bulk item needs from the node.
pub trait Targets {
    /// Whether `name` belongs to a plugin.
    fn is_system_index(&self, name: &str) -> bool;

    /// Where writes to `name` land, if it exists.
    ///
    /// # Errors
    ///
    /// A register that cannot be read.
    fn write_target(&self, name: &str) -> Result<Option<Target>, PlaceError>;

    /// The shard of `target` that `id` (or its routing) belongs to.
    ///
    /// # Errors
    ///
    /// The message of a mapping that requires routing when none was given.
    fn shard_for(&self, target: &Target, id: &str, routing: Option<&str>) -> Result<u32, String>;
}

/// What routing needs to know of one item.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ItemInput {
    /// The index the item names, or none.
    pub index: Option<String>,
    /// The document id.
    pub id: String,
    /// The item's custom routing.
    pub routing: Option<String>,
    /// Whether the item already failed (parsing, say).
    pub failed: bool,
    /// Whether the item is a create.
    pub create: bool,
    /// Whether the item is a delete.
    pub deletion: bool,
}

/// An item that cannot be written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ItemFailure {
    /// The HTTP status the item reports.
    pub status: u16,
    /// The error type the item reports.
    pub kind: String,
    /// The reason the item reports.
    pub reason: String,
}

/// Where an item lands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Routed {
    /// The backing index.
    pub index: String,
    /// Its uuid.
    pub uuid: String,
    /// The shard.
    pub shard: u32,
}

/// The items of one `index[shard]`, by item position.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Group {
    /// The backing index.
    pub index: String,
    /// Its uuid.
    pub uuid: String,
    /// The shard.
    pub shard: u32,
    /// The positions of the group's items, in item order.
    pub items: Vec<usize>,
}

fn fail(status: u16, kind: &str, reason: String) -> ItemFailure {
    ItemFailure {
        status,
        // ALLOC: one error type per failed item.
        kind: kind.to_owned(),
        reason,
    }
}

/// Routes every item: `None` for one that had already failed, otherwise
/// where it lands or why it cannot. Each name is looked up once.
pub fn route_items<T: Targets>(
    plane: &T,
    items: &[ItemInput],
) -> Vec<Option<Result<Routed, ItemFailure>>> {
    // ALLOC: one memo entry for each distinct index in the request.
    let mut memo: HashMap<&str, Result<Option<Target>, ()>> = HashMap::new();
    items
        .iter()
        .map(|item| {
            if item.failed {
                return None;
            }
            Some(route_one(plane, item, &mut memo))
        })
        // ALLOC: one outcome for each item of the request.
        .collect()
}

fn route_one<'a, T: Targets>(
    plane: &T,
    item: &'a ItemInput,
    memo: &mut HashMap<&'a str, Result<Option<Target>, ()>>,
) -> Result<Routed, ItemFailure> {
    let name = match item.index.as_deref() {
        Some(name) if !name.trim().is_empty() => name,
        _ => {
            return Err(fail(
                400,
                "missing_index",
                // ALLOC: one reason per failed item.
                "no index given for this item and none in the path".to_owned(),
            ));
        }
    };
    if plane.is_system_index(name) {
        // The path a request arrived on says nothing about what a bulk body
        // touches, so this is where a plugin's index is guarded.
        return Err(fail(
            403,
            "system_index",
            // ALLOC: one reason per failed item.
            format!("[{name}] belongs to a plugin and is not reachable through the request path"),
        ));
    }
    let looked = memo
        .entry(name)
        .or_insert_with(|| plane.write_target(name).map_err(|_| ()));
    let target = match looked {
        Err(()) => {
            return Err(fail(
                503,
                "metadata_unavailable",
                // ALLOC: one reason per failed item.
                format!("could not resolve index [{name}]; retry"),
            ));
        }
        Ok(None) => {
            return Err(fail(
                404,
                "index_not_found",
                // ALLOC: one reason per failed item.
                format!("no such index: {name}"),
            ));
        }
        Ok(Some(target)) => target,
    };
    if target.data_stream && (!item.create || item.deletion) {
        return Err(fail(
            400,
            "illegal_argument_exception",
            // ALLOC: one reason per failed item.
            format!(
                "only write ops with an op_type of create are allowed in data streams; \
                 address the backing index [{}] for anything else",
                target.index
            ),
        ));
    }
    let shard = plane
        .shard_for(target, &item.id, item.routing.as_deref())
        .map_err(|message| fail(400, "routing_missing_exception", message))?;
    Ok(Routed {
        // ALLOC: one name and uuid for each routed item.
        index: target.index.clone(),
        uuid: target.uuid.clone(),
        shard,
    })
}

/// Collects the routed items into one group for each `index[shard]`, in the
/// order first seen; items that were not routed are in none.
#[must_use]
pub fn group_by_shard(routed: &[Option<Result<Routed, ItemFailure>>]) -> Vec<Group> {
    // ALLOC: one entry for each distinct index and shard in the request.
    let mut at: HashMap<(&str, u32), usize> = HashMap::new();
    // ALLOC: one group for each distinct index and shard in the request.
    let mut groups: Vec<Group> = Vec::new();
    for (position, outcome) in routed.iter().enumerate() {
        let Some(Ok(r)) = outcome else { continue };
        let slot = *at.entry((r.index.as_str(), r.shard)).or_insert_with(|| {
            groups.push(Group {
                // ALLOC: one name and uuid for each group.
                index: r.index.clone(),
                uuid: r.uuid.clone(),
                shard: r.shard,
                // ALLOC: one position for each item of the group.
                items: Vec::new(),
            });
            groups.len().saturating_sub(1)
        });
        if let Some(group) = groups.get_mut(slot) {
            // ALLOC: one position for each routed item.
            group.items.push(position);
        }
    }
    groups
}
