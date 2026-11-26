//! Writing one window: the epoch, admissions, the blob, its outcome.

use std::sync::atomic::Ordering;
use std::sync::{Arc, PoisonError};
use std::time::Instant;

use oss_format::ShardBatch;
use oss_format::node_log_format::encode;

use super::timings::add_nanos;
use super::{CommitError, Flushing, NodeLogCommitter, Position};
use crate::log::epochs::EpochState;
use crate::log::window::Window;
use crate::log::{HedgeOutcome, LogError};

impl NodeLogCommitter {
    /// Writes one window. Called with the lock released, by the one
    /// flusher, which owns the window meanwhile.
    pub(super) fn write(&self, head: &mut Flushing) {
        let window = &mut head.window;
        // The window's store requests are its writers', not the thread's
        // that drives it.
        let _scope = oss_blob::cost::enter_scope(window.scope());
        let opening = Instant::now();
        let opened = self.open_epoch();
        add_nanos(&self.timings.epoch_nanos, opening.elapsed());
        let epoch = match opened {
            Ok(epoch) => epoch,
            Err(e) => {
                window.fail_all(&CommitError::Lease(e));
                return;
            }
        };
        let admitting = Instant::now();
        // A shard's first records in this epoch wait for a check that this
        // node still owns it; under one hold of the lock for the window.
        // ALLOC: one entry a (shard, term) new to the epoch, bounded by the window.
        let mut to_admit = Vec::new();
        {
            let inner = self.lock();
            if let Some(state) = inner.epochs.get(&epoch) {
                for ((shard, term), _) in &window.batches {
                    if !window.failures.contains_key(shard)
                        && state.admitted.get(shard) != Some(term)
                    {
                        to_admit.push((shard.clone(), *term));
                    }
                }
            }
        }
        self.timings.admission_checks.fetch_add(
            u64::try_from(to_admit.len()).unwrap_or(u64::MAX),
            Ordering::Relaxed,
        );
        for (shard, term) in to_admit {
            match self.admission.admit(&shard, term, epoch) {
                Ok(()) => {
                    if let Some(state) = self.lock().epochs.get_mut(&epoch) {
                        state.admitted.insert(shard, term);
                    }
                }
                Err(e) => {
                    window.failures.insert(shard, CommitError::Refused(e));
                }
            }
        }
        // ALLOC: one batch a (shard, term) in the window, borrowing its records.
        let batches: Vec<ShardBatch<'_>> = window
            .batches
            .iter()
            .filter(|((shard, _), _)| !window.failures.contains_key(shard))
            .map(|((shard, term), records)| ShardBatch {
                shard,
                term: *term,
                records,
            })
            // ALLOC: one batch a (shard, term) in the window, borrowing its records.
            .collect();
        add_nanos(&self.timings.admission_nanos, admitting.elapsed());
        if batches.is_empty() {
            return;
        }
        let (ordinal, trim_point, prior) = {
            let inner = self.lock();
            match inner.epochs.get(&epoch) {
                Some(state) => (
                    state.next,
                    state.trim_point(),
                    // ALLOC: the epoch's index, one entry a shard with unpublished records.
                    state
                        .unpublished
                        .iter()
                        .map(|(k, runs)| (k.clone(), runs.clone()))
                        .collect::<Vec<_>>(),
                ),
                None => return,
            }
        };
        let encoded = match encode(
            self.store.node_id(),
            epoch,
            ordinal,
            trim_point,
            &batches,
            &prior,
        ) {
            Ok(encoded) => encoded,
            Err(e) => {
                // Nothing was written, so the epoch is still good.
                let failure = CommitError::Encode(e);
                drop(batches);
                window.fail_all(&failure);
                return;
            }
        };
        let blob: Arc<[u8]> = Arc::from(encoded.bytes);
        let len = u64::try_from(blob.len()).unwrap_or(u64::MAX);
        let spawn = self
            .hedge_spawn
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let putting = Instant::now();
        let appended = self.store.append_hedged(
            epoch,
            ordinal,
            &blob,
            self.hedge_delay.delay(len),
            spawn.as_deref(),
        );
        match appended {
            Ok(HedgeOutcome::None) => {}
            Ok(outcome) => {
                self.hedges.fetch_add(1, Ordering::Relaxed);
                if outcome == HedgeOutcome::Won {
                    self.hedges_won.fetch_add(1, Ordering::Relaxed);
                }
            }
            Err(_) => {}
        }
        let took = putting.elapsed();
        add_nanos(&self.timings.put_nanos, took);
        self.timings.puts.fetch_add(1, Ordering::Relaxed);
        if appended.is_ok() {
            self.hedge_delay.note_put_took(took, len);
        }
        let landed_records: u64 = batches
            .iter()
            .map(|b| u64::try_from(b.records.len()).unwrap_or(u64::MAX))
            .fold(0, u64::saturating_add);
        // ALLOC: the shards the blob carried, for the index below.
        let shards: Vec<_> = batches.iter().map(|b| b.shard.clone()).collect();
        drop(batches);
        let mut inner = self.lock();
        let mut ended = false;
        if let Some(state) = inner.epochs.get_mut(&epoch) {
            // A fence in the slot means the blob did not land. Any other
            // failure may have landed it, so its records count as
            // unpublished until the shards publish past it.
            if !matches!(appended, Err(LogError::Taken)) {
                track(state, shards, ordinal);
            }
            if appended.is_ok() {
                state.next = ordinal.saturating_add(1);
            } else {
                // Closed to this writer, by a fence or an outcome it cannot
                // know. The next window opens a new epoch.
                state.writing = false;
                ended = true;
            }
        }
        if ended {
            inner.current = None;
        }
        drop(inner);
        match appended {
            Ok(_) => {
                head.landed = Some(Position { epoch, ordinal });
                self.blobs.fetch_add(1, Ordering::Relaxed);
                self.records.fetch_add(landed_records, Ordering::Relaxed);
            }
            Err(LogError::Taken) => head
                .window
                .fail_all(&CommitError::Fenced { epoch, ordinal }),
            Err(e) => head.window.fail_all(&CommitError::Store(e)),
        }
    }

    /// The epoch being written, recording a new one in the lease first if
    /// there is none.
    fn open_epoch(&self) -> Result<u64, super::Refusal> {
        let (epoch, recorded) = {
            let mut inner = self.lock();
            if let Some(current) = inner.current {
                return Ok(current);
            }
            let epoch = inner.next_epoch;
            // Named by positions() from here, so a renewal that writes the
            // lease meanwhile cannot drop it.
            inner.opening = Some(epoch);
            (epoch, Self::positions_locked(&inner))
        };
        let recorded_ok = self.lease.record(&recorded);
        let mut inner = self.lock();
        inner.opening = None;
        recorded_ok?;
        inner.next_epoch = epoch.saturating_add(1);
        inner
            .epochs
            .insert(epoch, crate::log::epochs::EpochState::default());
        inner.current = Some(epoch);
        self.timings.epochs_opened.fetch_add(1, Ordering::Relaxed);
        Ok(epoch)
    }

    /// Ends the epoch a write panicked in, whose blob may have landed: the
    /// window's shards count as unpublished at the slot it was aimed at,
    /// and the next window opens a new epoch.
    pub(super) fn abandon(&self, window: &Window) {
        let mut inner = self.lock();
        inner.opening = None;
        let Some(epoch) = inner.current.take() else {
            return;
        };
        if let Some(state) = inner.epochs.get_mut(&epoch) {
            state.writing = false;
            let ordinal = state.next;
            track(
                state,
                window.batches.iter().map(|((shard, _), _)| shard.clone()),
                ordinal,
            );
        }
    }
}

/// Counts `shards` as holding unpublished records at `ordinal`.
fn track(
    state: &mut EpochState,
    shards: impl IntoIterator<Item = oss_format::ShardKey>,
    ordinal: u64,
) {
    for shard in shards {
        let runs = state.unpublished.entry(shard).or_default();
        // An ordinal not past the shard's last run is there already.
        if let Ok(more) = oss_format::with_ordinal(runs, ordinal) {
            *runs = more;
        }
    }
}
