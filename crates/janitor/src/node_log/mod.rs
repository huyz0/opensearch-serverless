//! A dead node's write log: deleted once nothing in it is unpublished.
//!
//! Ported from the shell's `ShardSettlement.collectNodeLog`.

mod seams;

pub use seams::{Commits, Committed, FencedLog, NodeLogs};

use oss_cluster::Membership;
use oss_cluster::heads::ShardHeadStore;
use oss_format::ShardKey;
use oss_meta::DescriptorStore;

use crate::JanitorError;

/// What collecting one node's log did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Collected {
    /// The node has no lease, names no epochs, or is live.
    Nothing,
    /// Something in the log is not yet published, or the lease moved.
    StillNeeded,
    /// A dry run found the log collectable and left it.
    WouldCollect,
    /// The lease forgot the epochs and the log was deleted.
    Collected,
}

/// Collects dead nodes' write logs.
pub struct Collector<'a> {
    pub(crate) membership: &'a Membership,
    pub(crate) heads: &'a ShardHeadStore,
    pub(crate) descriptors: &'a DescriptorStore,
    pub(crate) logs: &'a dyn NodeLogs,
    pub(crate) commits: &'a dyn Commits,
    pub(crate) clock: &'a dyn Fn() -> i64,
    pub(crate) dry_run: bool,
}

impl<'a> Collector<'a> {
    /// A collector over the deployment's leases, heads and descriptors,
    /// reading logs and commits through the seams, judging liveness by
    /// `clock`.
    pub fn new(
        membership: &'a Membership,
        heads: &'a ShardHeadStore,
        descriptors: &'a DescriptorStore,
        logs: &'a dyn NodeLogs,
        commits: &'a dyn Commits,
        clock: &'a dyn Fn() -> i64,
        dry_run: bool,
    ) -> Self {
        Self {
            membership,
            heads,
            descriptors,
            logs,
            commits,
            clock,
            dry_run,
        }
    }

    /// Collects `node`'s log if it is dead and fully published.
    ///
    /// # Errors
    ///
    /// [`JanitorError`] for a failed read, fence or swap, or a delete that
    /// failed after the lease let the epochs go.
    pub fn collect(&self, node: &str, buf: &mut [u8]) -> Result<Collected, JanitorError> {
        let Some(lease) = self.membership.read(node)? else {
            return Ok(Collected::Nothing);
        };
        let live = !lease.revoked && !lease.is_expired_at((self.clock)());
        if lease.node_log.is_empty() || live {
            return Ok(Collected::Nothing);
        }
        let epochs = lease.node_log;
        // Every epoch is fenced first, so a node that is not as dead as it
        // looks adds nothing after what is read.
        let log = self.logs.fence(node, &epochs)?;
        for shard in log.named()? {
            if !self.published_past(node, &shard, log.as_ref(), buf)? {
                return Ok(Collected::StillNeeded);
            }
        }
        if self.dry_run {
            return Ok(Collected::WouldCollect);
        }
        let ends = log.ends()?;
        // The lease stops naming the epochs by a swap a returning node wins;
        // only then do the blobs go.
        if !self.membership.forget_node_log(node, &epochs)? {
            return Ok(Collected::StillNeeded);
        }
        for epoch in &epochs {
            let (Ok(number), Ok(from)) = (u64::try_from(epoch.epoch), u64::try_from(epoch.trim))
            else {
                continue;
            };
            let landed = u64::try_from(epoch.landed).unwrap_or(0);
            let end = ends.get(&number).copied().unwrap_or(landed);
            // From the trim point the lease recorded: the node deleted below
            // it itself, up to a renewal's worth.
            self.logs
                .delete_range(node, number, from, end.saturating_add(1))?;
        }
        Ok(Collected::Collected)
    }

    /// Whether `shard`'s published commit holds everything `log` holds of it.
    fn published_past(
        &self,
        node: &str,
        shard: &ShardKey,
        log: &dyn FencedLog,
        buf: &mut [u8],
    ) -> Result<bool, JanitorError> {
        let Some(name) = shard.index_name.as_deref() else {
            return Ok(false);
        };
        match self.descriptors.get(name, buf)? {
            // Deleted, or recreated under the name: nothing of this
            // incarnation is read again.
            None => return Ok(true),
            Some(record) if *record.uuid != *shard.index_uuid => return Ok(true),
            Some(_) => {}
        }
        // A head still the dead node's own: settled or taken first, and
        // judged then.
        if self
            .heads
            .read(name, shard.shard)?
            .is_some_and(|head| head.owner_node_id.as_deref() == Some(node))
        {
            return Ok(false);
        }
        let held = log.max_seq_no_of(shard)?;
        if held < 0 {
            return Ok(true);
        }
        Ok(
            match self
                .commits
                .committed(name, &shard.index_uuid, shard.shard)?
            {
                Committed::Absent | Committed::Commit(None) => false,
                Committed::Commit(Some(checkpoint)) => checkpoint >= held,
            },
        )
    }
}
