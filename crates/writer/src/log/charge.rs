//! Which indices a log blob's cost belongs to.
//!
//! One blob carries a window's records for every shard, so its PUT is shared
//! by the indices in it, by the bytes of their slices, as the shell's node
//! log splits it. Reading the weights needs the blob's header, so it is
//! done only when a store usage ledger is on.

use std::collections::BTreeMap;
use std::sync::Arc;

use oss_blob::usage::Ledger;
use oss_format::NodeLogEntry;
use oss_format::node_log_format::decode;

/// The indices of a blob and the bytes of its slices in each.
pub(super) type Weights = Arc<[(Arc<str>, u64)]>;

/// The ledger a log's PUTs are shared in.
#[derive(Clone)]
pub(super) struct Charge(pub(super) Arc<Ledger>);

impl Charge {
    /// The weights of `blob`: its slices' bytes by index name. A fence, a
    /// header that does not decode and a slice naming no index weigh nothing,
    /// so what is left to the node is what no index owns.
    pub(super) fn weights(blob: &[u8]) -> Weights {
        let Ok(NodeLogEntry::Records(header)) = decode(blob) else {
            return Weights::from([]);
        };
        // ALLOC: one entry an index in the blob, for one PUT.
        let mut by_name: BTreeMap<Arc<str>, u64> = BTreeMap::new();
        for slice in &header.slices {
            if let Some(name) = &slice.shard.index_name {
                let weight = by_name.entry(Arc::clone(name)).or_insert(0);
                *weight = weight.saturating_add(u64::from(slice.length));
            }
        }
        by_name.into_iter().collect() // ALLOC: the same entries, once a PUT.
    }

    /// Runs `work`, its requests shared by `weights`.
    pub(super) fn run<T>(&self, weights: &Weights, work: impl FnOnce() -> T) -> T {
        // ALLOC: one borrowed entry an index, for one PUT.
        let shares: Vec<(&str, u64)> = weights.iter().map(|(n, w)| (n.as_ref(), *w)).collect();
        self.0.split(&shares, work)
    }
}
