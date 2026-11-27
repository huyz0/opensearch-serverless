//! Adopting a shard reads only the log past its commit and replays in a
//! bounded buffer.

use crate::common;
use std::sync::Arc;

use common::committer::{rec, rig, shard, window};
use common::takeover::*;
use lucene_store::ByteBuffersDirectory;
use oss_blob::BlobStore;
use oss_writer::publish::PublishError;
use oss_writer::takeover::{HeadWriters, ShardEngine, adopt};

/// An engine that records what reaches it.
struct Recording {
    through: i64,
    seen: Vec<String>,
    buffered: usize,
    flushes: Vec<usize>,
    /// What a flush leaves buffered.
    residual: usize,
}

impl oss_writer::takeover::Engine for Recording {
    fn committed_through(&self) -> i64 {
        self.through
    }

    fn replay(&mut self, record: &oss_format::WalRecord<'_>) -> Result<bool, String> {
        self.seen.push(record.id().to_owned());
        self.buffered = self.buffered.saturating_add(100);
        Ok(true)
    }

    fn buffered_bytes(&self) -> usize {
        self.buffered
    }

    fn flush(&mut self) -> Result<(), String> {
        self.flushes.push(self.seen.len());
        self.buffered = self.residual;
        Ok(())
    }

    fn publish(&mut self, _term: u64) -> Result<oss_writer::publish::Published, PublishError> {
        Err(PublishError::Directory("not here".to_owned()))
    }
}

#[test]
fn only_records_above_the_commit_reach_the_engine_oldest_first() {
    let r = rig(window(0));
    write_three(&r, 1);
    write_three(&r, 2);
    let won = head(1, 2, &["node-1"]);
    let preds = predecessors(&r, &Arc::default());
    let heads = HeadWriters::new(Arc::clone(&r.store) as Arc<dyn BlobStore>, "");
    let mut engine = Recording {
        through: 0,
        seen: Vec::new(),
        buffered: 0,
        flushes: Vec::new(),
        residual: 0,
    };
    // The publish is refused, which ends the adopt after the replay.
    let _ = adopt(&shard(1), &won, &preds, &mut engine, &heads, usize::MAX);
    assert_eq!(
        engine.seen,
        ["a1", "a2"],
        "shard 1's records past seq 0 only"
    );
    assert!(engine.flushes.is_empty(), "below the bound: no flush");
}

#[test]
fn the_engine_is_flushed_when_its_buffer_reaches_the_bound() {
    let r = rig(window(0));
    write_three(&r, 1);
    let won = head(1, 2, &["node-1"]);
    let preds = predecessors(&r, &Arc::default());
    let heads = HeadWriters::new(Arc::clone(&r.store) as Arc<dyn BlobStore>, "");
    let mut engine = Recording {
        through: -1,
        seen: Vec::new(),
        buffered: 0,
        flushes: Vec::new(),
        residual: 0,
    };
    // 100 bytes a record, a bound of 200: flushed before the third.
    let _ = adopt(&shard(1), &won, &preds, &mut engine, &heads, 200);
    assert_eq!(engine.seen, ["a0", "a1", "a2"]);
    assert_eq!(engine.flushes, [2], "once, after two records");
}

/// Adopts shard 1's three records through a recording engine under `bound`.
fn adopt_three(bound: usize, residual: usize, writes: usize) -> Recording {
    let r = rig(window(0));
    for seq in 0..writes {
        r.committer
            .commit(&shard(1), 1, &[rec(&format!("w{seq}"), seq as i64)])
            .expect("lands");
    }
    let won = head(1, 2, &["node-1"]);
    let preds = predecessors(&r, &Arc::default());
    let heads = HeadWriters::new(Arc::clone(&r.store) as Arc<dyn BlobStore>, "");
    let mut engine = Recording {
        through: -1,
        seen: Vec::new(),
        buffered: 0,
        flushes: Vec::new(),
        residual,
    };
    let _ = adopt(&shard(1), &won, &preds, &mut engine, &heads, bound);
    engine
}

#[test]
fn a_bound_of_zero_still_makes_progress_and_applies_every_record() {
    let e = adopt_three(0, 0, 3);
    assert_eq!(e.seen, ["w0", "w1", "w2"]);
    assert_eq!(
        e.flushes,
        [1, 2],
        "a flush before each record after the first"
    );
}

#[test]
fn a_flush_that_leaves_more_than_the_bound_does_not_flush_before_every_record() {
    // 100 B a record, a bound of 250, 500 B left by each flush.
    let e = adopt_three(250, 500, 6);
    assert_eq!(e.seen.len(), 6);
    assert_eq!(e.flushes, [3], "one flush, then growth of 250 B is awaited");
}

/// A real engine that notes the most it ever buffered.
struct Peak<'e, 'a, 'd> {
    inner: ShardEngine<'a, 'd>,
    peak: &'e std::cell::Cell<usize>,
}

impl oss_writer::takeover::Engine for Peak<'_, '_, '_> {
    fn committed_through(&self) -> i64 {
        self.inner.committed_through()
    }

    fn replay(&mut self, record: &oss_format::WalRecord<'_>) -> Result<bool, String> {
        let applied = self.inner.replay(record)?;
        self.peak
            .set(self.peak.get().max(self.inner.buffered_bytes()));
        Ok(applied)
    }

    fn buffered_bytes(&self) -> usize {
        self.inner.buffered_bytes()
    }

    fn flush(&mut self) -> Result<(), String> {
        self.inner.flush()
    }

    fn publish(&mut self, term: u64) -> Result<oss_writer::publish::Published, PublishError> {
        self.inner.publish(term)
    }
}

#[test]
fn a_long_replay_holds_a_bounded_buffer_and_still_applies_and_publishes_every_record() {
    let run = |limit: usize| {
        let r = rig(window(0));
        for seq in 0..200 {
            r.committer
                .commit(&shard(1), 1, &[rec(&format!("d{seq}"), seq)])
                .expect("lands");
        }
        let won = head(1, 2, &["node-1"]);
        put_head(&r, &won);
        let preds = predecessors(&r, &Arc::default());
        let dir = ByteBuffersDirectory::new();
        let mut s = new_shard(&dir, 2);
        let mut p = publisher(&r, 1);
        let heads = HeadWriters::new(Arc::clone(&r.store) as Arc<dyn BlobStore>, "");
        let peak = std::cell::Cell::new(0);
        let mut engine = Peak {
            inner: ShardEngine::new(&mut s, &dir, &mut p, "node-2", 1),
            peak: &peak,
        };
        let done = adopt(&shard(1), &won, &preds, &mut engine, &heads, limit).expect("adopts");
        assert_eq!(done.replayed, 200);
        assert!(done.published.is_some(), "one publish");
        assert_eq!(registered_term(&r, 1), 2);
        peak.get()
    };
    let unbounded = run(usize::MAX);
    let bounded = run(8 * 1024);
    assert!(
        bounded < unbounded / 2,
        "bounded {bounded} B against unbounded {unbounded} B"
    );
    // The bound plus what one more record costs to buffer.
    assert!(
        bounded <= 8 * 1024 + unbounded / 200 + 1,
        "bounded {bounded} B"
    );
}
