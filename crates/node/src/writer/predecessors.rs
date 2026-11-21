//! Reading a predecessor's write log as its lease names it.
//!
//! The Java reads the previous writer's lease for the epochs of its log and
//! fences them; this does the same for each node a shard's head names.

use std::sync::Arc;

use oss_blob::BlobStore;
use oss_format::{NodeLease, NodeLogEpoch, lease_key};
use oss_writer::log::{NodeLogStore, NodeLogTakeover, Refusal, TakeoverError};
use oss_writer::takeover::{HeadWriters, Predecessors};

use super::Takeover;

/// The most a dead node's log header may be, in bytes: a successor keeps
/// every header it fences in memory, so a blob that claims more is refused
/// and its shards are left to the lazy take.
pub const MAX_HEADER_BYTES: u64 = 8 << 20;

/// The most a lease register holds.
const LEASE_BYTES: usize = 1 << 16;

/// The epochs the lease register of `node` names now; `None` when it has no
/// register.
pub(super) fn lease_epochs(
    store: &dyn BlobStore,
    base: &str,
    node: &str,
) -> Result<Option<Vec<NodeLogEpoch>>, Refusal> {
    let key = lease_key(base, node).map_err(|e| -> Refusal { Arc::new(e) })?;
    // ALLOC: one buffer a lease read, once a predecessor a batch.
    let mut buf = vec![0u8; LEASE_BYTES];
    let Some(read) = store
        .read_register(&key, &mut buf)
        .map_err(|e| -> Refusal { Arc::new(e) })?
    else {
        return Ok(None);
    };
    let bytes = buf.get(..read.len).unwrap_or_default();
    NodeLease::parse(bytes)
        .map(|lease| Some(lease.node_log))
        .map_err(|e| -> Refusal { Arc::new(e) })
}

impl Takeover {
    /// A takeover by node `node_id` (its fence recorded at `term`) that
    /// reads each predecessor's log from the epochs its lease register names
    /// and checks every fence against that lease again. A node with no lease
    /// register has no log to read, and the head keeps naming it.
    #[must_use]
    pub fn from_leases(
        store: Arc<dyn BlobStore>,
        base: &str,
        node_id: &str,
        term: i64,
        max_buffer_bytes: usize,
    ) -> Self {
        // ALLOC: two names, once a node.
        let base = base.to_owned();
        let me = node_id.to_owned();
        let heads = HeadWriters::new(Arc::clone(&store), &base);
        let make_store = Arc::clone(&store);
        let predecessors = Predecessors::new(Box::new(move |writer| {
            let Some(epochs) =
                lease_epochs(make_store.as_ref(), &base, writer).map_err(TakeoverError::Lease)?
            else {
                return Ok(None);
            };
            let log = NodeLogStore::new(Arc::clone(&make_store), base.clone(), writer)
                .map_err(TakeoverError::Log)?
                .with_max_header(MAX_HEADER_BYTES);
            let again = Arc::clone(&make_store);
            let again_base = base.clone();
            // ALLOC: one name, once a predecessor a batch.
            let again_node = writer.to_owned();
            Ok(Some(Arc::new(NodeLogTakeover::rereading(
                log,
                epochs,
                &me,
                term,
                Box::new(move || {
                    Ok(lease_epochs(again.as_ref(), &again_base, &again_node)?.unwrap_or_default())
                }),
            ))))
        }));
        Self {
            predecessors: Arc::new(predecessors),
            heads,
            max_buffer_bytes,
        }
    }
}
