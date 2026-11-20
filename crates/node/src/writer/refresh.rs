//! A refresh: the engine's writes become visible, and where the role
//! publishes, the commit that holds them is published before it returns, so
//! a reader that asks afterwards finds them.

use oss_writer::bulk::{IndexInfo, Owner};

use super::{WriterRole, lock};

impl WriterRole {
    /// Refreshes a shard on this node when it holds it, and otherwise asks
    /// the node that owns it to.
    ///
    /// # Errors
    ///
    /// As [`WriterRole::refresh`], or the owner not answering.
    pub fn refresh_owned(&self, index: &IndexInfo, shard: u32) -> Result<(), String> {
        match self.owner(index, shard) {
            Owner::Local => self.refresh(index, shard),
            Owner::Node(node) => self.routes.refresh_remote(&node, index, shard),
        }
    }

    /// Refreshes a shard held here and, where the role publishes, publishes
    /// its commit now.
    ///
    /// # Errors
    ///
    /// The shard not held, or the engine's or the publish's.
    pub fn refresh(&self, index: &IndexInfo, shard: u32) -> Result<(), String> {
        let Some(slot) = self.slot(&index.uuid, shard) else {
            // ALLOC: error path; one message.
            return Err(format!(
                "[{}][{shard}] is not open on this node",
                index.name
            ));
        };
        let mut held = lock(&slot);
        held.refresh()?;
        let Some(publishing) = &self.publishing else {
            return Ok(());
        };
        // ALLOC: one key a refresh.
        let key = (index.uuid.clone(), shard);
        let max_seq_no = held.max_seq_no();
        let done = publishing.with_publisher(&key, &index.name, |publisher, node, now| {
            held.publish(publisher, node, now)
        })?;
        publishing.published(&key, max_seq_no);
        drop(held);
        self.staged(&index.uuid, shard, &done.manifest);
        Ok(())
    }
}
