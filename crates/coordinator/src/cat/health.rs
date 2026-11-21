//! The `_cat/health` row.

use super::table::Table;
use crate::cluster::health::Health;

/// The one-row `_cat/health` table for `cluster`: the status word, the node
/// count as total and as data nodes, and the active and dormant shard counts,
/// each `-` when no index was asked about.
#[must_use]
pub fn row(cluster: &str, health: &Health) -> Table<6> {
    let mut table = Table::new([
        "cluster",
        "status",
        "node.total",
        "node.data",
        "shards",
        "dormant",
    ]);
    // ALLOC: the cells of one row: six short strings.
    let nodes = health.nodes.to_string();
    let (shards, dormant) = if health.indices.is_empty() {
        // ALLOC: two one-character cells.
        ("-".to_owned(), "-".to_owned())
    } else {
        // ALLOC: two short cells.
        (
            health.active_shards.to_string(),
            health.dormant_shards.to_string(),
        )
    };
    // ALLOC: the six cells of the one row.
    table.row([
        cluster.to_owned(),
        health.status.as_str().to_owned(),
        nodes.clone(),
        nodes,
        shards,
        dormant,
    ]);
    table
}
