//! Pieces of the overlay builder: one build at a time for each shard, and a
//! held record as the engine reads it.

use std::collections::HashSet;
use std::sync::{Condvar, Mutex, PoisonError};

use oss_format::WalRecord;
use oss_transport::actions::tail::Record;

/// The shards a build is running for. Two builds of one shard would both
/// reserve its estimate from the pool, which refuses the second.
#[derive(Default)]
pub(crate) struct Flights {
    running: Mutex<HashSet<(String, u32)>>,
    done: Condvar,
}

/// A shard's build in progress; dropped, it lets the next one start.
pub(crate) struct Flight<'a> {
    flights: &'a Flights,
    key: (String, u32),
}

impl Flights {
    /// Waits for any build of `key` to finish, then claims the shard's build.
    pub(crate) fn begin(&self, key: &(String, u32)) -> Flight<'_> {
        let mut running = self.running.lock().unwrap_or_else(PoisonError::into_inner);
        while running.contains(key) {
            running = self
                .done
                .wait(running)
                .unwrap_or_else(PoisonError::into_inner);
        }
        // ALLOC: one key a build running, bounded by the builds running.
        running.insert(key.clone());
        Flight {
            flights: self,
            key: key.clone(),
        }
    }
}

impl Drop for Flight<'_> {
    fn drop(&mut self) {
        self.flights
            .running
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&self.key);
        self.flights.done.notify_all();
    }
}

/// A held record as the engine reads it.
pub(crate) fn wal(r: &Record) -> WalRecord<'_> {
    if r.deletion {
        WalRecord::deletion(r.id.as_str(), r.seq_no, r.primary_term, r.version)
    } else {
        WalRecord::index(
            r.id.as_str(),
            r.source.as_str(),
            r.seq_no,
            r.primary_term,
            r.version,
        )
    }
}
