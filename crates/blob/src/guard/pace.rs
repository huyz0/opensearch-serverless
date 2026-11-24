//! Level three: a tenant's writes held to what fits its budget.

use super::{Guardrails, Mode, PoisonError};

/// Pacing lets a tenant run ahead of its rate by this many seconds of it.
const BURST_SECONDS: f64 = 4.0;

/// A write refused because holding it would take too long.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Refused {
    /// How long it would have waited, in milliseconds.
    pub retry_after_millis: u64,
}

impl Guardrails {
    /// How long to hold a write of `docs` documents before applying it, in
    /// milliseconds: zero unless the tenant is enforced at level three and
    /// over its budget. Call before anything is applied.
    ///
    /// # Errors
    ///
    /// [`Refused`] when the wait would pass the node's maximum; the write is
    /// not counted, and the tenant shows level four for a minute.
    pub fn pace_write(
        &self,
        mode: Mode,
        tenant: &str,
        docs: u64,
        now: i64,
    ) -> Result<u64, Refused> {
        if docs == 0 || !self.enforced_at(mode, tenant, 3, now) {
            return Ok(0);
        }
        let mut tenants = self.tenants.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(t) = tenants.get_mut(tenant) else {
            return Ok(0);
        };
        if t.allowed_docs_per_second.is_infinite() {
            return Ok(0);
        }
        let rate = t.allowed_docs_per_second.max(0.01);
        let docs = docs as f64;
        let capacity = docs.max(rate * BURST_SECONDS);
        t.tokens = match t.tokens_at {
            None => capacity,
            Some(at) => {
                let seconds = now.saturating_sub(at).max(0) as f64 / 1_000.0;
                capacity.min(t.tokens + seconds * rate)
            }
        };
        t.tokens_at = Some(now);
        t.tokens -= docs;
        let wait = if t.tokens >= 0.0 {
            0
        } else {
            (-t.tokens / rate * 1_000.0).ceil() as u64
        };
        if wait > self.max_wait_millis {
            t.tokens += docs;
            t.refused = t.refused.saturating_add(1);
            t.refused_at = Some(now);
            return Err(Refused {
                retry_after_millis: wait,
            });
        }
        if wait > 0 {
            t.paced = t.paced.saturating_add(1);
        }
        Ok(wait)
    }
}
