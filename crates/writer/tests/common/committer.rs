//! A committer on the emulator with a recording lease and admission.

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Barrier, Mutex};
use std::time::Duration;

use oss_blob::{BlobStore, Emulator, S3Op};
use oss_format::{NodeLogEntry, NodeLogEpoch, ShardKey, WalRecord};
use oss_writer::log::{
    CommitError, CommitterConfig, EpochLease, NodeLogCommitter, NodeLogStore, Position, Refusal,
    ShardAdmission,
};

use super::emulated;

/// A lease that records every write, failing while `fail` is set.
#[derive(Default)]
pub struct Lease {
    pub writes: Mutex<Vec<Vec<NodeLogEpoch>>>,
    pub fail: AtomicBool,
}

impl EpochLease for Lease {
    fn record(&self, epochs: &[NodeLogEpoch]) -> Result<(), Refusal> {
        if self.fail.load(Ordering::SeqCst) {
            return Err(Arc::new(std::io::Error::other("lease write failed")));
        }
        self.writes
            .lock()
            .expect("unpoisoned")
            .push(epochs.to_vec());
        Ok(())
    }
}

/// An admission that records every check and refuses the shards named.
#[derive(Default)]
pub struct Admission {
    pub checks: Mutex<Vec<(String, u32, i64, u64)>>,
    pub refuse: Mutex<BTreeSet<String>>,
}

impl ShardAdmission for Admission {
    fn admit(&self, shard: &ShardKey, term: i64, epoch: u64) -> Result<(), Refusal> {
        self.checks.lock().expect("unpoisoned").push((
            shard.index_uuid.to_string(),
            shard.shard,
            term,
            epoch,
        ));
        if self
            .refuse
            .lock()
            .expect("unpoisoned")
            .contains(shard.index_uuid.as_ref())
        {
            return Err(Arc::new(std::io::Error::other("not the owner")));
        }
        Ok(())
    }
}

pub struct Rig {
    pub store: Arc<Emulator>,
    pub log: NodeLogStore,
    pub lease: Arc<Lease>,
    pub admission: Arc<Admission>,
    pub committer: Arc<NodeLogCommitter>,
}

pub fn rig_on(store: Arc<Emulator>, log: NodeLogStore, config: CommitterConfig) -> Rig {
    let lease = Arc::new(Lease::default());
    let admission = Arc::new(Admission::default());
    let committer = Arc::new(NodeLogCommitter::new(
        log.clone(),
        7,
        Box::new(Arc::clone(&lease)),
        Box::new(Arc::clone(&admission)),
        config,
    ));
    Rig {
        store,
        log,
        lease,
        admission,
        committer,
    }
}

pub fn rig(config: CommitterConfig) -> Rig {
    let (store, log) = emulated();
    rig_on(store, log, config)
}

pub fn window(ms: u64) -> CommitterConfig {
    CommitterConfig {
        window: Duration::from_millis(ms),
        ..CommitterConfig::default()
    }
}

pub fn shard(n: u32) -> ShardKey {
    ShardKey::new(format!("uuid-{n}"), n, None::<&str>)
}

pub fn rec(id: &str, seq: i64) -> WalRecord<'static> {
    WalRecord::index(id.to_owned(), format!("{{\"n\":{seq}}}"), seq, 1, 1)
}

pub fn puts(r: &Rig) -> u64 {
    r.store.meter().count(S3Op::Put)
}

pub fn header_at(r: &Rig, at: Position) -> oss_format::NodeLogHeader<'static> {
    match r.log.read_entry(at.epoch, at.ordinal).expect("reads") {
        Some(NodeLogEntry::Records(h)) => h,
        other => panic!("expected records at {at:?}, got {other:?}"),
    }
}

/// Commits one record for each of `shards` from as many threads at once.
pub fn commit_all(r: &Rig, shards: u32) -> Vec<Result<Option<Position>, CommitError>> {
    let start = Arc::new(Barrier::new(usize::try_from(shards).expect("fits")));
    let threads: Vec<_> = (0..shards)
        .map(|n| {
            let (committer, start) = (Arc::clone(&r.committer), Arc::clone(&start));
            std::thread::spawn(move || {
                start.wait();
                committer.commit(&shard(n), 1, &[rec(&format!("doc-{n}"), i64::from(n))])
            })
        })
        .collect();
    threads
        .into_iter()
        .map(|t| t.join().expect("ends"))
        .collect()
}
