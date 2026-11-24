//! A writer role stopping: every held shard is published, put to sleep and
//! its head given back, so the next node takes it without waiting for this
//! node's lease.

use super::{HibernatePass, WriterRole, now_millis};

impl WriterRole {
    /// Publishes every shard that owes a commit, puts each held shard to
    /// sleep (an idle time of 0) and gives back the heads, passes repeated
    /// until no record is left, and all of it stopping at `deadline_millis`
    /// of the wall clock. The answer sums the passes: shards slept, heads
    /// given back, shards busy (written to meanwhile, pinned, or whose head
    /// had moved: left held), the shards whose publish failed (left held,
    /// their head kept), and the shards not reached before the deadline.
    pub fn retire(&self, now: u64, deadline_millis: u64) -> HibernatePass {
        let mut total = self.hibernate_idle_until(0, now, deadline_millis);
        loop {
            let before = self.hibernated();
            if before == 0 {
                return total;
            }
            if now_millis() >= deadline_millis {
                total.unfinished = total.unfinished.saturating_add(before);
                return total;
            }
            let given = self.give_back_idle(0, now);
            // A pass takes the record of every shard it examines, given back
            // or not, so the set shrinks and the loop ends.
            let examined = before.saturating_sub(self.hibernated());
            total.given_back = total.given_back.saturating_add(given);
            total.busy = total.busy.saturating_add(examined.saturating_sub(given));
            if examined == 0 {
                return total;
            }
        }
    }
}
