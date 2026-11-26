//! A restore that answers once its descriptors exist and copies on a thread
//! of its own (ADR 0063).
//!
//! The request thread creates every marked descriptor, so a name taken in
//! between is the caller's 400 and a search straight after the answer is
//! 503, never 404. The thread then copies and unmarks each index. Whatever
//! ends it short of that, a failure, a cancel at shutdown or a panic,
//! deletes what the restore created while the bytes it reserved are held.

use std::sync::{Arc, Mutex, PoisonError};

use oss_coordinator::snapshot_restore_admin::{AliasPlan, Target};
use oss_coordinator::snapshot_take_admin::ACCEPTED;
use oss_format::RestoreIndex;
use oss_meta::{CopyBuffers, new_id};
use oss_rest::{JSON, Response, RestError};

use crate::restore_background::{BackgroundRestores, Refusal, Slot};
use crate::route_util::refuse;
use crate::snapshot_restore_route::{SnapshotRestoreService, restore_failed, unavailable};

/// Bytes the descriptor's buffer holds beyond the snapshot record.
const DESCRIPTOR_SLACK: usize = 4096;

/// One index a background restore made: its name, uuid and generation.
struct Made {
    name: String,
    uuid: String,
    generation: u64,
}

/// What the thread owns: the slot, and what is deleted unless it finishes.
struct Undo {
    service: Arc<SnapshotRestoreService>,
    restores: Arc<BackgroundRestores>,
    made: Vec<Made>,
    armed: bool,
    _slot: Slot,
}

impl Undo {
    fn remove_all(service: &SnapshotRestoreService, made: &[Made]) {
        for m in made {
            // The failure being reported is the first.
            let _ = service.deletes.remove(&m.name, &m.uuid);
        }
    }
}

impl Drop for Undo {
    fn drop(&mut self) {
        if self.armed {
            Self::remove_all(&self.service, &self.made);
            self.restores.record_failure();
        }
    }
}

/// Everything the thread needs, handed over once.
struct Job {
    undo: Undo,
    aliases: Vec<AliasPlan>,
    truncated: Option<Vec<String>>,
    sources: Vec<RestoreIndex>,
    descriptor: Vec<u8>,
    now: i64,
}

fn refusal(r: Refusal) -> RestError {
    match r {
        // ALLOC: error path; one message.
        Refusal::Draining => refuse(
            503,
            "node_closing_exception",
            "this node is shutting down and starts no restore",
        ),
        Refusal::Busy => refuse(
            429,
            "es_rejected_execution_exception",
            "too many restores are running; retry when one has finished",
        ),
        Refusal::Pool(e) => RestError::circuit_breaking(&e, "<restore>"),
    }
}

impl SnapshotRestoreService {
    /// Creates the marked descriptor of every `sources` entry, answers
    /// accepted and copies on a thread. Every check that precedes a write
    /// has run; `record_len` sizes the descriptor's buffer.
    pub(crate) fn restore_in_background(
        self: &Arc<Self>,
        restores: &Arc<BackgroundRestores>,
        resp: &mut Response<'_>,
        targets: &[Target],
        sources: Vec<RestoreIndex>,
        (record_len, (aliases, truncated)): (usize, (Vec<AliasPlan>, Option<Vec<String>>)),
    ) -> Result<(), RestError> {
        let copy = self.chunk_bytes;
        let descriptor_bytes = record_len.saturating_add(DESCRIPTOR_SLACK);
        let held = copy
            .saturating_mul(2)
            .saturating_add(descriptor_bytes)
            // The sources the thread keeps are about the record's size.
            .saturating_add(record_len)
            .saturating_add(self.deletes.remove_bytes())
            .saturating_add(if truncated.is_some() {
                self.alias_bytes()
            } else {
                0
            });
        let slot = restores.acquire(held).map_err(refusal)?;
        // ALLOC: one buffer a restore, reserved by the slot.
        let mut descriptor = vec![0u8; descriptor_bytes];
        let now = i64::try_from((self.clock)()).unwrap_or(i64::MAX);
        let mut undo = Undo {
            service: Arc::clone(self),
            restores: Arc::clone(restores),
            // ALLOC: one entry an index, at most the take's bound.
            made: Vec::with_capacity(targets.len()),
            armed: false,
            _slot: slot,
        };
        for (target, source) in targets.iter().zip(&sources) {
            if restores.cancelled() {
                Undo::remove_all(self, &undo.made);
                return Err(refusal(Refusal::Draining));
            }
            let uuid = new_id().map_err(|e| {
                Undo::remove_all(self, &undo.made);
                unavailable(&e)
            })?;
            let created = self.restorer.create_marked(
                &self.descriptors,
                source,
                &target.target,
                &uuid,
                now,
                &mut descriptor,
            );
            match created {
                Ok(generation) => undo.made.push(Made {
                    name: target.target.clone(), // ALLOC: one name an index.
                    uuid,
                    generation,
                }),
                Err(e) => {
                    Undo::remove_all(self, &undo.made);
                    return Err(restore_failed(e, &target.target));
                }
            }
        }
        // ALLOC: one cell for the hand-over, a restore.
        let cell = Arc::new(Mutex::new(Some(Job {
            undo,
            aliases,
            truncated,
            sources,
            descriptor,
            now,
        })));
        let handed = Arc::clone(&cell);
        let me = Arc::clone(self);
        let spawned = std::thread::Builder::new()
            .name("snapshot-restore".to_owned())
            .spawn(move || {
                let taken = handed.lock().unwrap_or_else(PoisonError::into_inner).take();
                if let Some(job) = taken {
                    me.finish_job(job);
                }
            });
        if spawned.is_err() {
            let taken = cell.lock().unwrap_or_else(PoisonError::into_inner).take();
            if let Some(job) = taken {
                Undo::remove_all(self, &job.undo.made);
            }
            return Err(refuse(
                503,
                "node_unavailable",
                "this node cannot start a thread for the restore",
            ));
        }
        resp.send(200, JSON, ACCEPTED.as_bytes())
    }

    /// The thread's work: copy and unmark each index, in order. Ends with
    /// the undo disarmed only when every index finished.
    fn finish_job(&self, job: Job) {
        let Job {
            mut undo,
            aliases,
            truncated,
            sources,
            mut descriptor,
            now,
        } = job;
        undo.armed = true;
        let restores = Arc::clone(&undo.restores);
        let copy = self.chunk_bytes;
        // ALLOC: two buffers a restore, `chunk_bytes` each, reserved by the slot.
        let (mut read, mut write) = (vec![0u8; copy], vec![0u8; copy]);
        let cancel = || restores.cancelled();
        for (made, source) in undo.made.iter().zip(&sources) {
            let done = self.restorer.finish_index(
                &self.descriptors,
                source,
                (&made.name, &made.uuid),
                (now, made.generation),
                &mut descriptor,
                &mut CopyBuffers {
                    read: &mut read,
                    write: &mut write,
                },
                &cancel,
            );
            if let Err(e) = done {
                eprintln!("snapshot restore of [{}] failed: {e}", made.name);
                return;
            }
        }
        // Every index is whole: from here nothing the restore made is deleted,
        // whatever the alias step does.
        undo.armed = false;
        if let Some(truncated) = truncated {
            let report = self.add_aliases(&aliases, truncated, &cancel);
            if !report.failed.is_empty() {
                eprintln!(
                    "snapshot restore could not add aliases {:?}; the indices are restored",
                    report.failed
                );
                undo.restores.record_failure();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oss_blob::{BlobStore, Emulator};
    use oss_format::IndexRecord;
    use oss_mem::{BudgetConfig, MemoryBudget, Pool};
    use oss_meta::{DeleteMode, DescriptorStore};

    use crate::index_delete::{DeleteService, NoSnapshots};
    use crate::restore_background::Settings;

    fn service() -> (Arc<SnapshotRestoreService>, DescriptorStore) {
        let blobs: Arc<dyn BlobStore> = Arc::new(Emulator::new(1 << 24, 1));
        let clock: oss_meta::PitClock = Arc::new(|| 1_700_000_000_000);
        let descriptors =
            || DescriptorStore::new(Arc::clone(&blobs), "", DeleteMode::Conditional).unwrap();
        let deletes = Arc::new(
            DeleteService::new(
                &blobs,
                "",
                DeleteMode::Conditional,
                Arc::clone(&clock),
                Arc::new(NoSnapshots),
                1 << 16,
                8,
            )
            .unwrap(),
        );
        let service = SnapshotRestoreService::new(
            Arc::clone(&blobs),
            "",
            descriptors(),
            deletes,
            clock,
            true,
            64,
        );
        (Arc::new(service), descriptors())
    }

    // A thread that panics mid-copy still deletes what the restore made and
    // gives back its place and bytes: the guard does it in its drop.
    #[test]
    fn a_restore_thread_that_panics_deletes_its_indices_and_frees_its_slot() {
        let (service, descriptors) = service();
        let uuid = "u1".to_owned();
        let created = descriptors
            .create(
                &IndexRecord::new("copy", &uuid, 1),
                1,
                &mut vec![0u8; 1 << 16],
            )
            .unwrap();
        let budget = MemoryBudget::new(
            &BudgetConfig::new(1 << 20)
                .with_pool(Pool::RequestBodies, 1 << 10)
                .unwrap(),
        );
        let restores = BackgroundRestores::new(
            Settings {
                enabled: true,
                max: 1,
                grace: std::time::Duration::from_millis(10),
            },
            budget.clone(),
        );
        let slot = restores.acquire(512).expect("fits");
        let undo = Undo {
            service,
            restores: Arc::clone(&restores),
            made: vec![Made {
                name: "copy".into(),
                uuid,
                generation: created,
            }],
            armed: true,
            _slot: slot,
        };
        let ended = std::thread::spawn(move || {
            let _undo = undo;
            panic!("a copy failed");
        })
        .join();
        assert!(ended.is_err());
        assert!(
            descriptors
                .get("copy", &mut vec![0u8; 1 << 16])
                .unwrap()
                .is_none()
        );
        assert_eq!(restores.failed(), 1);
        assert_eq!(restores.running(), 0);
        assert_eq!(budget.in_use(Pool::RequestBodies), 0);
    }

    // The same guard, not armed, deletes nothing and counts nothing.
    #[test]
    fn a_finished_restore_leaves_its_indices_and_counts_no_failure() {
        let (service, descriptors) = service();
        let generation = descriptors
            .create(
                &IndexRecord::new("copy", "u1", 1),
                1,
                &mut vec![0u8; 1 << 16],
            )
            .unwrap();
        let budget = MemoryBudget::new(
            &BudgetConfig::new(1 << 20)
                .with_pool(Pool::RequestBodies, 1 << 10)
                .unwrap(),
        );
        let restores = BackgroundRestores::new(Settings::off(), budget);
        let slot = restores.acquire(8).expect("fits");
        drop(Undo {
            service,
            restores: Arc::clone(&restores),
            made: vec![Made {
                name: "copy".into(),
                uuid: "u1".into(),
                generation,
            }],
            armed: false,
            _slot: slot,
        });
        assert!(
            descriptors
                .get("copy", &mut vec![0u8; 1 << 16])
                .unwrap()
                .is_some()
        );
        assert_eq!((restores.failed(), restores.running()), (0, 0));
    }
}
