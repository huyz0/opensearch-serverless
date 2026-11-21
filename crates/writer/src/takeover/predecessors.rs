//! The logs of the nodes a batch of takes replays, each made once.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError};

use oss_format::NodeLogEpoch;

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

    /// The log of `writer` as its lease names it now: the one made before
    /// when it was made from the same epochs, else a new one, so a node that
    /// died, came back under the same id with new epochs and died again is
    /// not read as it was.
    ///
    /// # Errors
    ///
    /// As [`Predecessors::of`].
    pub fn of_naming(
        &self,
        writer: &str,
        epochs: &[NodeLogEpoch],
    ) -> Result<Option<Arc<NodeLogTakeover>>, TakeoverError> {
        let stale = {
            let made = self.made.lock().unwrap_or_else(PoisonError::into_inner);
            made.get(writer)
                .and_then(Option::as_ref)
                .is_some_and(|log| !same_epochs(log.epochs(), epochs))
        };
        if stale {
            self.forget(writer);
        }
        self.of(writer)
    }

    /// Drops what was made of `writer`'s log: the next ask reads its lease
    /// and fences again. For a log that was collected.
    pub fn forget(&self, writer: &str) {
        self.made
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(writer);
    }
}

/// Whether two lease views name the same epochs; a later trim point or
/// landed hint of the same epoch is the same log.
fn same_epochs(a: &[NodeLogEpoch], b: &[NodeLogEpoch]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.epoch == y.epoch)
}
