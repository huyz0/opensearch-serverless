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

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use oss_blob::{BlobStore, Emulator};

    use super::*;
    use crate::log::NodeLogStore;

    fn epoch(epoch: i64, trim: i64) -> NodeLogEpoch {
        NodeLogEpoch {
            epoch,
            trim,
            landed: 0,
        }
    }

    /// Predecessors whose factory counts its calls and reads `epochs`.
    fn counting(epochs: Vec<NodeLogEpoch>) -> (Predecessors, Arc<AtomicU64>) {
        let calls = Arc::new(AtomicU64::new(0));
        let seen = Arc::clone(&calls);
        let store: Arc<dyn BlobStore> = Arc::new(Emulator::new(1 << 20, 1));
        let p = Predecessors::new(Box::new(move |writer| {
            seen.fetch_add(1, Ordering::SeqCst);
            let log = NodeLogStore::new(Arc::clone(&store), "", writer).expect("log");
            Ok(Some(Arc::new(NodeLogTakeover::new(
                log,
                epochs.clone(),
                "node-2",
                2,
            ))))
        }));
        (p, calls)
    }

    #[test]
    fn a_log_named_by_the_same_epochs_is_the_one_made_before() {
        let (p, calls) = counting(vec![epoch(1, 0)]);
        let first = p.of("node-1").expect("made").expect("a log");
        // A later trim point of the same epoch is the same log.
        let again = p
            .of_naming("node-1", &[epoch(1, 5)])
            .expect("made")
            .expect("a log");
        assert!(Arc::ptr_eq(&first, &again));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_log_named_by_other_epochs_is_made_again() {
        let (p, calls) = counting(vec![epoch(1, 0)]);
        let first = p.of("node-1").expect("made").expect("a log");
        let other = p
            .of_naming("node-1", &[epoch(2, 0)])
            .expect("made")
            .expect("a log");
        assert!(!Arc::ptr_eq(&first, &other));
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        let more = p
            .of_naming("node-1", &[epoch(2, 0), epoch(3, 0)])
            .expect("made")
            .expect("a log");
        assert!(!Arc::ptr_eq(&other, &more), "a longer list is another log");
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn naming_a_node_not_made_yet_makes_it_once() {
        let (p, calls) = counting(vec![epoch(1, 0)]);
        let log = p
            .of_naming("node-1", &[epoch(1, 0)])
            .expect("made")
            .expect("a log, not none");
        assert_eq!(log.predecessor(), "node-1");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn forgetting_a_node_makes_it_again_on_the_next_ask() {
        let (p, calls) = counting(vec![epoch(1, 0)]);
        p.of("node-1").expect("made");
        p.of("node-1").expect("known");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        p.forget("node-1");
        p.of("node-1").expect("made");
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn epoch_lists_are_the_same_when_their_epoch_numbers_are() {
        assert!(same_epochs(&[], &[]));
        assert!(same_epochs(
            &[epoch(1, 0), epoch(2, 3)],
            &[epoch(1, 9), epoch(2, 0)]
        ));
        assert!(!same_epochs(&[epoch(1, 0)], &[epoch(2, 0)]));
        assert!(!same_epochs(
            &[epoch(1, 0), epoch(2, 0)],
            &[epoch(1, 0), epoch(3, 0)]
        ));
        assert!(!same_epochs(&[epoch(1, 0)], &[epoch(1, 0), epoch(2, 0)]));
        assert!(!same_epochs(&[epoch(1, 0), epoch(2, 0)], &[epoch(1, 0)]));
    }
}
