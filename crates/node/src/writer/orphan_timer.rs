//! The timer that runs the orphan-shard sweep.
//!
//! One node of the fleet sweeps: the live ingest member that ranks first by
//! rendezvous on a fixed name, as the tombstone sweep does, so the others
//! make no request. The thread owns the one [`Sweeper`] for its life, because
//! the sweeper counts the failures of a page across runs, and runs a slice
//! once an interval. A budget of 0 starts nothing.

mod bucket_orphans;

pub use bucket_orphans::BucketMemberSweep;

use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::Duration;

use super::BucketWiring;
use oss_blob::BlobStore;
use oss_cluster::placement::reapers_for;
use oss_format::NodeLease;
use oss_janitor::orphans::{Config, PointInTimeSource, SnapshotSource, Sweeper};
use oss_mem::{MemoryBudget, Pool};
use oss_meta::{DeleteMode, DescriptorStore, PitClock, PointInTimeStore};

/// The name the sweeper's role is ranked by.
const ROLE: &str = "orphan-sweep";

/// The longest descriptor a slice reads.
const DESCRIPTOR_BYTES: usize = 1 << 16;

/// The longest snapshot record a slice reads.
const SNAPSHOT_RECORD_BYTES: usize = 1 << 16;

/// What a slice may hold at once: its descriptor buffer, one point-in-time
/// record (at most 1 MiB) and one snapshot record, with room to spare.
const SLICE_RESERVE: usize = 2 << 20;

/// The live members, as the node's lease thread last saw them.
#[derive(Default)]
pub struct LiveLeases(Mutex<Arc<[NodeLease]>>);

impl LiveLeases {
    /// Replaces the members.
    pub fn set(&self, live: Arc<[NodeLease]>) {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = live;
    }

    fn get(&self) -> Arc<[NodeLease]> {
        Arc::clone(&self.0.lock().unwrap_or_else(PoisonError::into_inner))
    }
}

/// What the timer reads and writes through.
pub struct OrphanParts {
    /// The store the containers and descriptors live in.
    pub store: Arc<dyn BlobStore>,
    /// The deployment's base path.
    pub base: String,
    /// The node sweeping.
    pub node_id: String,
    /// The memory a slice's buffers come from.
    pub budget: MemoryBudget,
    /// How many containers one slice judges; 0 starts nothing.
    pub slice: usize,
    /// How long between slices.
    pub every: Duration,
    /// Count what would be deleted and delete nothing.
    pub dry_run: bool,
    /// The clock the point-in-time records are judged by.
    pub clock: PitClock,
    /// The bucket wiring, when buckets are on: each slice then also judges a
    /// few buckets' members.
    pub buckets: Option<Arc<BucketWiring>>,
    /// How long a member must stay a candidate before it is taken out.
    pub bucket_grace_millis: i64,
}

/// Runs a slice every interval until dropped.
pub struct OrphanTimer {
    stop: Arc<(Mutex<bool>, Condvar)>,
    thread: Option<JoinHandle<()>>,
}

impl OrphanTimer {
    /// Starts the thread, or nothing for a slice budget of 0.
    pub fn spawn(parts: OrphanParts, live: Arc<LiveLeases>) -> Option<Self> {
        if parts.slice == 0 {
            return None;
        }
        let stop = Arc::new((Mutex::new(false), Condvar::new()));
        let signal = Arc::clone(&stop);
        let thread = std::thread::Builder::new()
            .name("orphan-sweep".to_owned())
            .spawn(move || run(&parts, &live, &signal))
            .ok()?;
        Some(Self {
            stop,
            thread: Some(thread),
        })
    }
}

fn run(parts: &OrphanParts, live: &LiveLeases, signal: &(Mutex<bool>, Condvar)) {
    let Ok(descriptors) =
        DescriptorStore::new(Arc::clone(&parts.store), &parts.base, DeleteMode::Tombstone)
    else {
        return;
    };
    let Ok(pits) = PointInTimeStore::new(
        Arc::clone(&parts.store),
        &parts.base,
        Arc::clone(&parts.clock),
    ) else {
        return;
    };
    let max_pins =
        usize::try_from(oss_janitor::settings::ORPHAN_MAX_PINS.default()).unwrap_or(usize::MAX);
    let max_records = usize::try_from(oss_janitor::settings::ORPHAN_MAX_PIN_RECORDS.default())
        .unwrap_or(usize::MAX);
    let point_in_time = PointInTimeSource::new(&pits, max_records);
    let snapshots = SnapshotSource::new(
        Arc::clone(&parts.store),
        &parts.base,
        max_records,
        SNAPSHOT_RECORD_BYTES,
    );
    let sweeper = Sweeper::new(
        parts.store.as_ref(),
        &descriptors,
        &parts.base,
        Config {
            dry_run: parts.dry_run,
            max_pins,
            descriptor_bytes: DESCRIPTOR_BYTES,
        },
        &point_in_time,
        &snapshots,
    );
    let bucket_sweep = parts.buckets.as_ref().and_then(|wiring| {
        let descriptors =
            DescriptorStore::new(Arc::clone(&parts.store), &parts.base, DeleteMode::Tombstone)
                .ok()?;
        Some(BucketMemberSweep::new(
            Arc::clone(wiring),
            Arc::clone(&parts.store),
            descriptors,
            &parts.base,
            parts.bucket_grace_millis,
            parts.dry_run,
        ))
    });
    let (flag, wake) = signal;
    loop {
        let stopped = flag.lock().unwrap_or_else(PoisonError::into_inner);
        // A wake that is not the drop's (spurious) runs the slice a little
        // early, which the sweep does not mind.
        let (stopped, _) = wake
            .wait_timeout(stopped, parts.every)
            .unwrap_or_else(PoisonError::into_inner);
        if *stopped {
            return;
        }
        drop(stopped);
        let members = live.get();
        if !reapers_for(ROLE, members.iter(), 1).contains(&parts.node_id.as_str()) {
            continue;
        }
        let Ok(_reserved) = parts.budget.try_reserve(Pool::RequestBodies, SLICE_RESERVE) else {
            continue;
        };
        // A failed slice leaves the cursor where it was and is tried again
        // at the next interval; the sweeper skips a page that keeps failing.
        let _ = sweeper.slice(parts.slice);
        if let Some(bucket_sweep) = &bucket_sweep {
            let now = i64::try_from((parts.clock)()).unwrap_or(i64::MAX);
            let _ = bucket_sweep.slice(now);
        }
    }
}

impl Drop for OrphanTimer {
    fn drop(&mut self) {
        let (flag, wake) = &*self.stop;
        *flag.lock().unwrap_or_else(PoisonError::into_inner) = true;
        wake.notify_all();
        if let Some(thread) = self.thread.take() {
            // A slice that panicked already ended the thread.
            let _ = thread.join();
        }
    }
}
