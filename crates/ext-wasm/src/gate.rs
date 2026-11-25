//! The concurrency gate: at most so many calls at once for a module and for
//! one tenant; a full gate answers at once, nothing queues.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

#[derive(Default)]
struct Counts {
    total: u32,
    tenants: HashMap<String, u32>,
}

/// The gate of one module.
pub(crate) struct Gate {
    total: u32,
    tenant: u32,
    counts: Arc<Mutex<Counts>>,
}

/// A held place; released on drop.
pub(crate) struct Permit {
    tenant: String,
    counts: Arc<Mutex<Counts>>,
}

fn lock(m: &Mutex<Counts>) -> MutexGuard<'_, Counts> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl Gate {
    pub(crate) fn new(total: u32, tenant: u32) -> Self {
        Self {
            total,
            tenant,
            counts: Arc::default(),
        }
    }

    /// A place for `tenant`, or `None` when the module or the tenant is full.
    pub(crate) fn enter(&self, tenant: &str) -> Option<Permit> {
        let mut c = lock(&self.counts);
        let mine = c.tenants.get(tenant).copied().unwrap_or(0);
        if c.total >= self.total || mine >= self.tenant {
            return None;
        }
        // ARITH: both are below their caps, which are u32 and far from the maximum.
        #[allow(clippy::arithmetic_side_effects)]
        {
            c.total += 1;
        }
        // ALLOC: one entry per tenant with a call in flight, removed when it ends.
        c.tenants.insert(tenant.to_owned(), mine.saturating_add(1));
        Some(Permit {
            tenant: tenant.to_owned(),
            counts: Arc::clone(&self.counts),
        })
    }
}

impl Drop for Permit {
    fn drop(&mut self) {
        let mut c = lock(&self.counts);
        c.total = c.total.saturating_sub(1);
        if let Some(n) = c.tenants.get_mut(&self.tenant) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                c.tenants.remove(&self.tenant);
            }
        }
    }
}
