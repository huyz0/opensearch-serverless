//! The logs of the nodes a batch of takes replays, each made once.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError};

use crate::log::{NodeLogTakeover, TakeoverError};

/// Makes the reader of one node's log: it reads the node's lease, so it is
/// what a batch of takes must not do once a shard. `None` when the node has
/// no lease left, and so no log to read.
pub type PredecessorsFactory =
    Box<dyn Fn(&str) -> Result<Option<Arc<NodeLogTakeover>>, TakeoverError> + Send + Sync>;

/// The predecessors a batch of takes shares: one [`NodeLogTakeover`] per
/// node, and with it one fence and one lease read, however many of the
/// shards taken name it.
pub struct Predecessors {
    make: PredecessorsFactory,
    made: Mutex<BTreeMap<String, Option<Arc<NodeLogTakeover>>>>,
}

impl Predecessors {
    /// Predecessors made by `make`.
    #[must_use]
    pub fn new(make: PredecessorsFactory) -> Self {
        Self {
            make,
            // ALLOC: one entry a node whose log the batch reads, a handful.
            made: Mutex::new(BTreeMap::new()),
        }
    }

    /// The log of `writer`, made on the first ask.
    ///
    /// # Errors
    ///
    /// When the factory fails; the next ask tries again.
    pub fn of(&self, writer: &str) -> Result<Option<Arc<NodeLogTakeover>>, TakeoverError> {
        let mut made = self.made.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(known) = made.get(writer) {
            return Ok(known.clone());
        }
        let log = (self.make)(writer)?;
        // ALLOC: the node's name, once a batch.
        made.insert(writer.to_owned(), log.clone());
        Ok(log)
    }
}
