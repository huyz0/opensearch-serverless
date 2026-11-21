//! The role's reads of a shard it holds: a document by id, and the index a
//! name resolves to.

use oss_writer::bulk::{IndexInfo, Owner};
use oss_writer::index::Fetched;

use super::{WriterRole, lock};

impl WriterRole {
    /// The live document of `id`, from this node when it holds the shard and
    /// otherwise from the node that owns it.
    ///
    /// # Errors
    ///
    /// As [`WriterRole::get`], or the owner not answering.
    pub fn get_owned(
        &self,
        index: &IndexInfo,
        shard: u32,
        id: &str,
        realtime: bool,
    ) -> Result<Option<Fetched>, String> {
        match self.owner(index, shard) {
            Owner::Local => self.get(index, shard, id, realtime),
            Owner::Node(node) => self
                .routes
                .get_remote_as(&node, index, shard, id, realtime)
                // ALLOC: error path; one message.
                .map_err(|e| format!("[{}][{shard}] is held by [{node}]: {e}", index.name)),
        }
    }

    /// The index called `name`, as the role's routes know it.
    pub fn index_info(&self, name: &str) -> Option<IndexInfo> {
        self.routes.index(name)
    }

    /// The live document of `id` in a shard held here; see
    /// [`oss_writer::index::Shard::get`].
    ///
    /// # Errors
    ///
    /// The shard not held, released, or the engine failing.
    pub fn get(
        &self,
        index: &IndexInfo,
        shard: u32,
        id: &str,
        realtime: bool,
    ) -> Result<Option<Fetched>, String> {
        let Some(slot) = self.slot(&index.uuid, shard) else {
            // ALLOC: error path; one message.
            return Err(format!(
                "[{}][{shard}] is not open on this node",
                index.name
            ));
        };
        let mut slot = lock(&slot);
        if slot.released() {
            // ALLOC: error path; one message.
            return Err(format!(
                "[{}][{shard}] was released after a failed write",
                index.name
            ));
        }
        slot.get(id, realtime)
    }
}
