//! A hedged append: the same PUT sent a second time when the first is slow.
//!
//! Both PUTs are create-if-absent of the same bytes at the same name, so at
//! most one lands, and a fence that takes the slot first makes both fail.
//! What a hedge has to tell apart is a slot taken by a fence from one taken
//! by its own other PUT, which answer the same way: so when both PUTs find
//! the slot taken, or one does and the other fails, the slot is read back,
//! and holds this blob exactly when one of them landed it. That costs one
//! GET, only when a hedge raced itself or a fence.
//!
//! A PUT that loses is left to finish on its own; nothing waits for it.

use std::sync::Arc;
use std::sync::mpsc;
use std::time::Duration;

use oss_blob::{BlobError, BlobStore, ByteRange};

use super::store::{IO_BUF_BYTES, create};
use super::{LogError, NodeLogStore};

/// What a hedged append did beyond its first PUT.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HedgeOutcome {
    /// The first PUT answered within the hedge delay, or failed; no second
    /// PUT was sent.
    None,
    /// A second PUT was sent, and the first answered first anyway.
    Sent,
    /// A second PUT was sent, and it answered first: the hedge cut the wait.
    Won,
}

/// Runs a hedged append's PUTs off the caller's thread (the Java's
/// `Executor`).
pub trait Spawn: Send + Sync {
    /// Runs `job` on another thread. `false` when it cannot, and `job` was
    /// dropped unrun.
    fn spawn(&self, job: Box<dyn FnOnce() + Send>) -> bool;
}

/// A thread a PUT: for tests and tools. A node's committer brings its own,
/// bounded.
#[derive(Clone, Copy, Debug, Default)]
pub struct ThreadPerPut;

impl Spawn for ThreadPerPut {
    fn spawn(&self, job: Box<dyn FnOnce() + Send>) -> bool {
        std::thread::Builder::new()
            .name("node-log-put".into())
            .spawn(job)
            .is_ok()
    }
}

/// Which PUT answered, and how.
type Answer = (Which, Result<(), LogError>);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Which {
    First,
    Second,
}

impl NodeLogStore {
    /// Writes a blob into a slot like [`NodeLogStore::append`], sending the
    /// same PUT a second time if the first has not answered within
    /// `hedge_after`, and returning as soon as either lands. A zero delay,
    /// or no `spawn`, sends one PUT on this thread.
    ///
    /// # Errors
    ///
    /// [`LogError::Taken`] if the slot is taken by anything but this blob:
    /// by a fence, so the epoch is closed. [`LogError::Blob`] if neither PUT
    /// landed or the outcome is unknown; the writer must not use the epoch
    /// again.
    pub fn append_hedged(
        &self,
        epoch: u64,
        ordinal: u64,
        blob: &Arc<[u8]>,
        hedge_after: Duration,
        spawn: Option<&dyn Spawn>,
    ) -> Result<HedgeOutcome, LogError> {
        let Some(spawn) = spawn.filter(|_| !hedge_after.is_zero()) else {
            self.append(epoch, ordinal, blob)?;
            return Ok(HedgeOutcome::None);
        };
        // ALLOC: the slot's key, shared by both PUTs.
        let key: Arc<str> = Arc::from(self.key(epoch, ordinal)?);
        // Room for both answers, so a PUT that loses never blocks sending.
        let (tx, rx) = mpsc::sync_channel::<Answer>(2);
        let put = |which: Which| -> bool {
            let (store, key, blob, tx) = (
                Arc::clone(&self.store),
                Arc::clone(&key),
                Arc::clone(blob),
                tx.clone(),
            );
            // ALLOC: one boxed job a PUT, at most two an append.
            spawn.spawn(Box::new(move || {
                let _ = tx.send((which, create(store.as_ref(), &key, &blob)));
            }))
        };
        if !put(Which::First) {
            return Err(LogError::Blob(BlobError::Unavailable));
        }
        // `tx` is held here, so the wait ends in an answer or a timeout.
        if let Ok((_, answer)) = rx.recv_timeout(hedge_after) {
            // Only one PUT was sent: a taken slot is taken by something
            // else, exactly as a plain append reports.
            return answer.map(|()| HedgeOutcome::None);
        }
        let sent_second = put(Which::Second);
        drop(tx);
        let (which, answer) = rx.recv().map_err(|_| lost())?;
        if answer.is_ok() {
            return Ok(outcome(which));
        }
        let first_failure = answer;
        // The one that answered failed: the other may still land.
        let second_failure = if sent_second {
            let (which, answer) = rx.recv().map_err(|_| lost())?;
            if answer.is_ok() {
                return Ok(outcome(which));
            }
            Some(answer)
        } else {
            None
        };
        let taken = |a: &Result<(), LogError>| matches!(a, Err(LogError::Taken));
        if taken(&first_failure) || second_failure.as_ref().is_some_and(taken) {
            // Taken: by this very blob, when one of the PUTs landed and did
            // not hear so; otherwise by a fence.
            return if holds(self.store.as_ref(), &key, blob)? {
                Ok(HedgeOutcome::Sent)
            } else {
                Err(LogError::Taken)
            };
        }
        first_failure.map(|()| HedgeOutcome::Sent)
    }
}

fn outcome(which: Which) -> HedgeOutcome {
    match which {
        Which::First => HedgeOutcome::Sent,
        Which::Second => HedgeOutcome::Won,
    }
}

/// A PUT whose thread ended without answering: its outcome is unknown.
fn lost() -> LogError {
    LogError::Blob(BlobError::Unavailable)
}

/// Whether `key` holds exactly `blob`: one GET, compared chunk by chunk.
fn holds(store: &dyn BlobStore, key: &str, blob: &[u8]) -> Result<bool, LogError> {
    let mut at = 0usize;
    let mut same = true;
    let read = store.get(
        key,
        ByteRange::Full,
        &mut [0u8; IO_BUF_BYTES],
        &mut |chunk: &[u8]| {
            let end = at.saturating_add(chunk.len());
            if blob.get(at..end) != Some(chunk) {
                // Not this blob: stop the download here.
                same = false;
                return Err(BlobError::Stream("the slot holds another blob"));
            }
            at = end;
            Ok(())
        },
    );
    match read {
        Ok(_) => Ok(at == blob.len()),
        Err(_) if !same => Ok(false),
        Err(BlobError::NotFound) => Ok(false),
        Err(e) => Err(e.into()),
    }
}
