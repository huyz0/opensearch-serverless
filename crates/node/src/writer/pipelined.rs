//! A write group that releases its shard's slot lock while its records wait
//! for the node log, so groups to one shard share a commit window.
//!
//! Above a depth of 1 a group applies and stages under the slot lock, waits
//! for its window without it, and finishes in the order it staged: the
//! effects of an acknowledgement run under the slot lock, in ticket order,
//! and only while no earlier group of the shard has failed.

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, MutexGuard};

use oss_writer::bulk::{IndexInfo, Planned};
use oss_writer::index::Written;
use oss_writer::log::Staged;

use super::apply_group::{Applied, apply_group};
use super::pipeline::{Pipeline, Quiet};
use super::slot::ShardSlot;
use super::{Failure, WriterRole, lock, unavailable};

/// Fails the pipeline and takes the group's turn out of it if the group
/// unwinds or returns before it finished.
struct Abort<'a> {
    role: &'a WriterRole,
    window: &'a Staged,
    pipe: &'a Arc<Pipeline>,
    held: &'a Arc<Mutex<ShardSlot>>,
    uuid: &'a str,
    shard: u32,
    ticket: u64,
    armed: bool,
}

impl Drop for Abort<'_> {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        // Its windows are driven to the end first, so none can land after a
        // reopen; this never unwinds.
        self.window.finish(&self.role.committer);
        self.pipe.fail();
        // Earlier groups finish under the slot lock, so this waits for them
        // without it.
        self.pipe.wait_turn(self.ticket);
        let mut slot = lock(self.held);
        if self.pipe.leave(self.ticket) {
            slot.release();
            drop(slot);
            self.role.release(self.uuid, self.shard, self.held);
        }
    }
}

impl WriterRole {
    /// The depth of the pipeline a shard's groups share: 1 holds the slot
    /// lock across the log wait, as before.
    pub fn set_pipeline_depth(&self, depth: usize) {
        self.pipeline_depth.store(depth.max(1), Ordering::Relaxed);
    }

    /// The depth set for the pipeline; 1 is the path without one.
    pub fn pipeline_depth(&self) -> usize {
        self.pipeline_depth.load(Ordering::Relaxed)
    }

    /// Applies an admitted group to the opened `slot` of the shard, letting
    /// others of the shard stage while it waits for the log.
    pub(super) fn group_pipelined(
        &self,
        held: &Arc<Mutex<ShardSlot>>,
        index: &IndexInfo,
        shard: u32,
        group: &[Planned<'_>],
    ) -> Vec<Result<Written, Failure>> {
        let pipe = lock(held).pipeline();
        let depth = self.pipeline_depth.load(Ordering::Relaxed);
        let mut slot = loop {
            let slot = lock(held);
            if slot.asleep() {
                // Slept after this write took the slot: wake the shard.
                drop(slot);
                return self.group(index, shard, group);
            }
            if slot.released() || pipe.failed() {
                return unavailable(group.len(), || {
                    // ALLOC: error path; one message.
                    format!(
                        "cannot index into [{}][{shard}]: released after a failed write",
                        index.name
                    )
                });
            }
            if pipe.must_wait(depth) {
                drop(slot);
                pipe.wait_ready(depth);
                continue;
            }
            break slot;
        };
        let Applied {
            mut outcomes,
            records,
            staged,
        } = apply_group(&mut slot, group);
        if records.is_empty() {
            return outcomes;
        }
        let first = staged
            .first()
            .and_then(|at| outcomes.get(*at))
            .and_then(|o| o.as_ref().ok())
            .map_or(0, |w| w.seq_no);
        let appended_at = self.limiter_now();
        // The descriptor holds the fields the group added before its records
        // are staged, so a reader never finds a document whose field it
        // cannot map.
        let staged_window = self.hold_growth(&mut slot, index).and_then(|()| {
            self.committer
                .stage(slot.key(), slot.term(), &records)
                .map_err(|e| e.to_string())
        });
        let window = match staged_window {
            Ok(Some(window)) => window,
            Ok(None) => return outcomes,
            Err(e) => {
                // The engine holds writes the log does not.
                pipe.fail();
                self.appended(appended_at, false);
                refuse(&mut outcomes, &staged, &e);
                if !pipe.busy() {
                    slot.release();
                    drop(slot);
                    self.release(&index.uuid, shard, held);
                }
                return outcomes;
            }
        };
        slot.stage(first);
        let ticket = pipe.enter();
        drop(slot);
        let mut abort = Abort {
            role: self,
            window: &window,
            pipe: &pipe,
            held,
            uuid: &index.uuid,
            shard,
            ticket,
            armed: true,
        };
        let committed = window.wait(&self.committer);
        pipe.wait_turn(ticket);
        let mut slot = lock(held);
        let earlier_failed = pipe.failed();
        self.appended(appended_at, committed.is_ok() && !earlier_failed);
        if committed.is_ok() && !earlier_failed {
            if let Some(p) = &self.publishing {
                // ALLOC: one key a staged group.
                p.written(&(index.uuid.clone(), shard));
            }
            if let Some(t) = self.tail.get() {
                // ALLOC: one owned copy an acknowledged record, the keep's.
                let owned: Vec<oss_format::WalRecord<'static>> = records
                    .iter()
                    .cloned()
                    .map(oss_format::WalRecord::into_owned)
                    .collect();
                t.acked(&index.name, &index.uuid, shard, &owned);
            }
        } else {
            pipe.fail();
            let why = match &committed {
                Err(e) => e.to_string(),
                Ok(_) => "an earlier write to the shard could not be made durable".to_owned(),
            };
            refuse(&mut outcomes, &staged, &why);
        }
        if committed.is_ok() && !earlier_failed {
            slot.acknowledge();
        }
        let last_out = pipe.leave(ticket);
        abort.armed = false;
        if last_out {
            slot.release();
            drop(slot);
            self.release(&index.uuid, shard, held);
        }
        outcomes
    }
}

/// Locks `slot` once none of its groups is in flight and holds new ones off
/// until the returned guard drops: what a publish, hibernation or eviction
/// needs, so no write the log may not hold ends up in a commit or is lost
/// to a sleeping shard. Writes wait on the pipeline, not on the slot lock.
pub(super) fn quiet_slot(slot: &Arc<Mutex<ShardSlot>>) -> (MutexGuard<'_, ShardSlot>, Quiet) {
    let pipe = lock(slot).pipeline();
    let quiet = pipe.quiesce();
    loop {
        let held = lock(slot);
        if !pipe.busy() {
            return (held, quiet);
        }
        drop(held);
        pipe.wait_idle();
    }
}

/// Marks the items that had a record as refused for `why`.
fn refuse(outcomes: &mut [Result<Written, Failure>], staged: &[usize], why: &str) {
    for at in staged {
        if let Some(o) = outcomes.get_mut(*at) {
            // ALLOC: error path; one message an item.
            *o = Err(Failure::Unavailable(format!(
                "the write could not be made durable: {why}"
            )));
        }
    }
}
