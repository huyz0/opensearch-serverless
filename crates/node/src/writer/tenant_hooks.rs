//! What a node's tenant levels hang on a writer role: who is told the
//! bucket group of an index when a shard of it is opened, and how far a
//! shard's publish age is stretched, and how long a group of writes waits.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use super::WriterRole;

/// Who is told an index's bucket group: index name and group.
pub type GroupSink = Arc<dyn Fn(&str, &str) + Send + Sync>;

/// What to do with a group of writes: the index's name and the documents in
/// it in; milliseconds to hold it, or the milliseconds a refused one would
/// have waited, out.
pub type WriteGate = Arc<dyn Fn(&str, usize) -> Result<u64, u64> + Send + Sync>;

/// What the tenant levels hang on a role.
#[derive(Default)]
pub(super) struct TenantHooks {
    groups: OnceLock<GroupSink>,
    gate: OnceLock<WriteGate>,
    /// Whole seconds the last refusal asked a writer to wait; 0 when the
    /// last write was let through.
    retry_secs: AtomicU64,
}

impl WriterRole {
    /// Installs who is told the bucket group of an index when a shard of it
    /// is opened. False when one is installed already.
    pub fn set_group_sink(&self, sink: GroupSink) -> bool {
        self.tenant.groups.set(sink).is_ok()
    }

    /// Tells the group sink that `index` bills to `group`.
    pub(crate) fn note_group(&self, index: &str, group: &str) {
        if let Some(sink) = self.tenant.groups.get() {
            sink(index, group);
        }
    }

    /// Installs how many times longer the writes of an index may wait to be
    /// published: the index's name in, the multiplier (1 for none) out.
    /// False when one is installed already, or this role does not publish.
    pub fn set_publish_stretch(&self, stretch: Arc<dyn Fn(&str) -> u64 + Send + Sync>) -> bool {
        let Some(publishing) = &self.publishing else {
            return false;
        };
        let names = Arc::clone(&self.names);
        publishing.set_stretch(Arc::new(move |uuid| {
            names.name_of(uuid).map_or(1, |index| stretch(&index))
        }))
    }

    /// Installs who decides, before a group of writes is applied, whether it
    /// waits or is refused. False when one is installed already.
    pub fn set_write_gate(&self, gate: WriteGate) -> bool {
        self.tenant.gate.set(gate).is_ok()
    }

    /// Asks the write gate about a group of `items` for `index`: sleeps the
    /// wait it names, or answers the reason a refused group fails with.
    pub(super) fn gate_write(&self, index: &str, items: usize) -> Result<(), String> {
        let Some(gate) = self.tenant.gate.get() else {
            return Ok(());
        };
        match gate(index, items) {
            Ok(wait) => {
                self.tenant.retry_secs.store(0, Ordering::Relaxed);
                if wait > 0 {
                    std::thread::sleep(Duration::from_millis(wait));
                }
                Ok(())
            }
            Err(wait) => {
                self.tenant
                    .retry_secs
                    .store(wait.div_ceil(1_000).max(1), Ordering::Relaxed);
                // ALLOC: error path; one message.
                Err(format!(
                    "[{index}] is over its cost budget; writes are paced, retry after {wait} ms"
                ))
            }
        }
    }

    /// The seconds the last refused write was told to wait, if it was.
    pub(super) fn gate_retry_secs(&self) -> Option<u64> {
        Some(self.tenant.retry_secs.load(Ordering::Relaxed)).filter(|s| *s > 0)
    }
}
