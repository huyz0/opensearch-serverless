//! The indexes a reader keeps open, the order they were last searched in,
//! and the first opens under way, so one index is opened by one search at a
//! time and the least recently searched is closed first
//! ([ADR 0014](../../../docs/adr/0014-open-indexes-pool.md)).

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};

/// One first open under way, which other searches of the index wait for.
#[derive(Debug, Default)]
pub struct Opening {
    done: Mutex<bool>,
    finished: Condvar,
}

impl Opening {
    /// Waits until the open finishes, opened or failed.
    pub fn wait(&self) {
        let mut done = self.done.lock().unwrap_or_else(PoisonError::into_inner);
        while !*done {
            done = self
                .finished
                .wait(done)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }

    fn finish(&self) {
        *self.done.lock().unwrap_or_else(PoisonError::into_inner) = true;
        self.finished.notify_all();
    }
}

/// What a search of an index finds.
pub enum Found<T> {
    /// The index is open; it is now the most recently searched.
    Open(Arc<T>),
    /// Another search is opening it: wait, then look again.
    Wait(Arc<Opening>),
    /// Nobody is: this search opens it, and calls [`OpenSet::finish`]
    /// when done, whatever the outcome.
    Lead,
}

/// The open indexes, each a `T`.
#[derive(Debug)]
pub struct OpenSet<T> {
    /// Each open index and the tick of its last search.
    indexes: HashMap<Arc<str>, (Arc<T>, u64)>,
    /// Each open index by the tick of its last search, oldest first.
    by_use: BTreeMap<u64, Arc<str>>,
    /// The first opens under way, at most one a search in flight.
    opening: HashMap<Arc<str>, Arc<Opening>>,
    /// The next tick.
    tick: u64,
}

impl<T> Default for OpenSet<T> {
    fn default() -> Self {
        Self {
            // ALLOC: one entry an open index; the open_indexes pool bounds them.
            indexes: HashMap::new(),
            // ALLOC: one entry an open index, as above.
            by_use: BTreeMap::new(),
            // ALLOC: one entry a search in flight, at most the server's workers.
            opening: HashMap::new(),
            tick: 0,
        }
    }
}

impl<T> OpenSet<T> {
    /// How many indexes are open.
    pub fn len(&self) -> usize {
        self.indexes.len()
    }

    /// Whether no index is open.
    pub fn is_empty(&self) -> bool {
        self.indexes.is_empty()
    }

    /// What a search of `index` finds; a [`Found::Lead`] marks the index as
    /// being opened.
    pub fn find(&mut self, index: &str) -> Found<T> {
        if let Some(held) = self.touch(index) {
            return Found::Open(held);
        }
        if let Some(opening) = self.opening.get(index) {
            return Found::Wait(Arc::clone(opening));
        }
        // ALLOC: the index name, once a first open; at most one a search in flight.
        self.opening.insert(Arc::from(index), Arc::default());
        Found::Lead
    }

    /// Ends the first open of `index` this search leads, adding `held` as
    /// the most recently searched when it opened, and wakes the searches
    /// waiting for it.
    pub fn finish(&mut self, index: &str, held: Option<Arc<T>>) {
        if let Some(held) = held {
            let tick = self.next_tick();
            let name = self
                .opening
                .get_key_value(index)
                .map(|(name, _)| Arc::clone(name))
                // ALLOC: the index name, once an open index.
                .unwrap_or_else(|| Arc::from(index));
            self.by_use.insert(tick, Arc::clone(&name));
            if let Some((_, old)) = self.indexes.insert(name, (held, tick)) {
                self.by_use.remove(&old);
            }
        }
        if let Some(opening) = self.opening.remove(index) {
            opening.finish();
        }
    }

    /// The total `size` of the open indexes no search holds: what closing
    /// every one of them would free.
    pub fn idle_bytes(&self, size: impl Fn(&T) -> usize) -> usize {
        self.indexes
            .values()
            .filter(|(held, _)| Arc::strong_count(held) == 1)
            .map(|(held, _)| size(held))
            .fold(0usize, usize::saturating_add)
    }

    /// Closes the least recently searched index that no search holds, and
    /// hands it back for the caller to drop once the set's lock is
    /// released; `None` when every open index is held or none is open.
    /// Closing a held index would free nothing until its searches end, and
    /// cost its next search a cold open.
    pub fn close_oldest(&mut self) -> Option<Arc<T>> {
        let idle = self.by_use.iter().find_map(|(tick, name)| {
            self.indexes
                .get(name)
                .filter(|(held, _)| Arc::strong_count(held) == 1)
                .map(|_| *tick)
        });
        let name = self.by_use.remove(&idle?)?;
        self.indexes.remove(&name).map(|(held, _)| held)
    }

    /// `index`, now the most recently searched, if it is open.
    fn touch(&mut self, index: &str) -> Option<Arc<T>> {
        let tick = self.next_tick();
        let (held, used) = self.indexes.get_mut(index)?;
        let held = Arc::clone(held);
        let old = std::mem::replace(used, tick);
        if let Some(name) = self.by_use.remove(&old) {
            self.by_use.insert(tick, name);
        }
        Some(held)
    }

    fn next_tick(&mut self) -> u64 {
        let tick = self.tick;
        self.tick = tick.saturating_add(1);
        tick
    }
}

/// Ends a first open when dropped, so a search that fails or panics while
/// opening never leaves the others waiting.
pub struct Lead<'a, T> {
    set: &'a Mutex<OpenSet<T>>,
    index: &'a str,
    held: Option<Arc<T>>,
}

impl<'a, T> Lead<'a, T> {
    /// The first open of `index` in `set`, which this search leads.
    pub fn new(set: &'a Mutex<OpenSet<T>>, index: &'a str) -> Self {
        Self {
            set,
            index,
            held: None,
        }
    }

    /// Records the opened index, added to the set when this ends.
    pub fn opened(mut self, held: Arc<T>) -> Arc<T> {
        self.held = Some(Arc::clone(&held));
        held
    }
}

impl<T> Drop for Lead<'_, T> {
    fn drop(&mut self) {
        lock(self.set).finish(self.index, self.held.take());
    }
}

/// Locks `set`, taking it over if a search panicked holding it: every
/// change to it is one whole step.
pub fn lock<T>(set: &Mutex<OpenSet<T>>) -> MutexGuard<'_, OpenSet<T>> {
    set.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_least_recently_found_closes_first() {
        let mut set: OpenSet<u8> = OpenSet::default();
        assert!(set.is_empty());
        for (i, name) in ["a", "b"].into_iter().enumerate() {
            assert!(matches!(set.find(name), Found::Lead));
            set.finish(name, Some(Arc::new(u8::try_from(i).expect("small"))));
        }
        assert!(!set.is_empty());
        assert!(matches!(set.find("a"), Found::Open(v) if *v == 0));
        assert!(set.close_oldest().is_some());
        assert_eq!(set.len(), 1);
        assert!(matches!(set.find("b"), Found::Lead), "b was closed");
        set.finish("b", None);
        assert!(matches!(set.find("a"), Found::Open(_)));
        assert!(set.close_oldest().is_some());
        assert!(set.close_oldest().is_none());
    }

    #[test]
    fn an_index_a_search_holds_is_not_closed() {
        let mut set: OpenSet<u8> = OpenSet::default();
        for name in ["a", "b"] {
            assert!(matches!(set.find(name), Found::Lead));
            set.finish(name, Some(Arc::new(0)));
        }
        let Found::Open(held) = set.find("a") else {
            panic!("a is open");
        };
        let Found::Open(_) = set.find("b") else {
            panic!("b is open");
        };
        assert_eq!(set.idle_bytes(|_| 10), 10, "only b is idle");
        // a is the oldest, but held: b closes instead.
        assert!(set.close_oldest().is_some());
        assert!(matches!(set.find("b"), Found::Lead), "b was closed");
        set.finish("b", None);
        assert!(set.close_oldest().is_none(), "only the held a is left");
        assert_eq!(set.len(), 1);
        drop(held);
        assert!(set.close_oldest().is_some());
        assert!(set.is_empty());
    }

    #[test]
    fn a_second_search_waits_for_the_first_open() {
        let set: Arc<Mutex<OpenSet<u8>>> = Arc::default();
        assert!(matches!(lock(&set).find("a"), Found::Lead));
        let Found::Wait(opening) = lock(&set).find("a") else {
            panic!("not waiting");
        };
        let lead = Lead::new(&set, "a");
        let (woken, ended) = std::sync::mpsc::sync_channel(1);
        let waiter = std::thread::spawn(move || {
            opening.wait();
            woken.send(()).expect("sends");
        });
        assert!(
            ended
                .recv_timeout(std::time::Duration::from_millis(100))
                .is_err(),
            "the waiter waits until the open ends"
        );
        assert_eq!(*lead.opened(Arc::new(7)), 7);
        ended.recv().expect("woken");
        waiter.join().expect("joins");
        assert!(matches!(lock(&set).find("a"), Found::Open(v) if *v == 7));
    }

    #[test]
    fn a_failed_open_lets_the_next_search_lead() {
        let set: Mutex<OpenSet<u8>> = Mutex::default();
        assert!(matches!(lock(&set).find("a"), Found::Lead));
        drop(Lead::new(&set, "a"));
        assert!(matches!(lock(&set).find("a"), Found::Lead));
        assert!(lock(&set).is_empty());
    }
}
