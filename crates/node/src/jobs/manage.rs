//! Extending, deleting and counting jobs.

use std::sync::atomic::Ordering;

use super::{JobError, Jobs, Stats, Who};

impl Jobs {
    /// Sets the expiry of job `num` to `keep_alive_ms` from now.
    ///
    /// # Errors
    ///
    /// 400 over the maximum; 404 when there is no such job for this caller.
    pub fn extend(&self, who: &Who, num: u64, keep_alive_ms: u64) -> Result<(), JobError> {
        if keep_alive_ms > self.limits.max_keep_alive_ms {
            return Err(JobError::BadRequest(format!(
                "Keep alive for asynchronous search ({keep_alive_ms}ms) is too large. It must be less than ({}ms)",
                self.limits.max_keep_alive_ms
            )));
        }
        let now = self.now();
        let mut inner = self.lock();
        self.sweep(&mut inner, now);
        let e = inner
            .entries
            .iter_mut()
            .find(|e| e.num == num && e.who == *who)
            .ok_or(JobError::NotFound)?;
        e.expires = now.saturating_add(keep_alive_ms);
        Ok(())
    }

    /// Cancels a running job and drops the entry, or drops a finished one.
    /// A cancelled search's thread keeps its slot and its reservation until it
    /// exits.
    ///
    /// # Errors
    ///
    /// 404 when there is no such job for this caller.
    pub fn delete(&self, who: &Who, num: u64) -> Result<(), JobError> {
        let now = self.now();
        let mut inner = self.lock();
        self.sweep(&mut inner, now);
        let at = inner
            .entries
            .iter()
            .position(|e| e.num == num && e.who == *who)
            .ok_or(JobError::NotFound)?;
        let e = inner.entries.remove(at);
        if e.done.is_none() {
            e.cancel.store(true, Ordering::Relaxed);
            inner.stats.cancelled = inner.stats.cancelled.saturating_add(1);
        }
        drop(inner);
        self.changed.notify_all();
        Ok(())
    }

    /// This node's counts.
    pub fn stats(&self) -> Stats {
        let mut inner = self.lock();
        self.sweep(&mut inner, self.now());
        let mut s = inner.stats;
        s.running_current = inner.threads as u64;
        s.held_bytes = self.budget.in_use(self.limits.pool) as u64;
        s
    }
}
