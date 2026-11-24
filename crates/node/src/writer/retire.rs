//! A writer role stopping: every held shard is published, put to sleep and
//! its head given back, so the next node takes it without waiting for this
//! node's lease.

use super::{GIVE_BACK_PER_PASS, HibernatePass, WriterRole};

impl WriterRole {
    /// Publishes every shard that owes a commit, puts each held shard to
    /// sleep (an idle time of 0) and gives back the heads, passes repeated
    /// until one gives none back. The answer sums the passes: shards slept,
    /// heads given back, shards busy (written to meanwhile, left held) and
    /// the shards whose publish failed (left held, their head kept).
    pub fn retire(&self, now_millis: u64) -> HibernatePass {
        let mut total = self.hibernate_idle(0, now_millis);
        loop {
            let given = self.give_back_idle(0, now_millis);
            total.given_back = total.given_back.saturating_add(given);
            if given < GIVE_BACK_PER_PASS {
                return total;
            }
        }
    }
}
