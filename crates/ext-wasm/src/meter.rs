//! What each tenant's module calls cost: the count of calls, the fuel they
//! ran, and the bytes that went in and came out.

use std::collections::HashMap;
use std::sync::Mutex;

/// Tenants counted by name; the ones past this fold into [`OTHER`], so the
/// meter's memory is bounded however many tenants call.
pub const MAX_TENANTS: usize = 4096;

/// The name the tenants past [`MAX_TENANTS`] are counted under.
pub const OTHER: &str = "_other";

/// One tenant's counters.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Usage {
    /// Calls made, finished or failed.
    pub calls: u64,
    /// Fuel the calls ran.
    pub fuel: u64,
    /// Bytes sent to modules.
    pub input_bytes: u64,
    /// Bytes modules answered with.
    pub output_bytes: u64,
}

/// The counters of every tenant of one host.
#[derive(Default)]
pub struct Meter {
    tenants: Mutex<HashMap<String, Usage>>,
}

impl Meter {
    /// Adds one call of `tenant`.
    pub(crate) fn record(&self, tenant: &str, fuel: u64, input: usize, output: usize) {
        let mut tenants = self
            .tenants
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let full = tenants.len() >= MAX_TENANTS && !tenants.contains_key(tenant);
        let key = if full { OTHER } else { tenant };
        let usage = match tenants.get_mut(key) {
            Some(usage) => usage,
            // ALLOC: one entry a tenant, at most MAX_TENANTS and the overflow entry.
            None => tenants.entry(key.to_owned()).or_default(),
        };
        usage.calls = usage.calls.saturating_add(1);
        usage.fuel = usage.fuel.saturating_add(fuel);
        usage.input_bytes = usage.input_bytes.saturating_add(input as u64);
        usage.output_bytes = usage.output_bytes.saturating_add(output as u64);
    }

    /// Every tenant's counters, by name.
    pub fn snapshot(&self) -> Vec<(String, Usage)> {
        let tenants = self
            .tenants
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // ALLOC: one entry a tenant, at most MAX_TENANTS and the overflow entry.
        let mut all: Vec<(String, Usage)> = tenants.iter().map(|(k, v)| (k.clone(), *v)).collect();
        all.sort_by(|a, b| a.0.cmp(&b.0));
        all
    }

    /// One tenant's counters; zero for a tenant not seen.
    pub fn usage(&self, tenant: &str) -> Usage {
        self.tenants
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(tenant)
            .copied()
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calls_add_up_by_tenant() {
        let m = Meter::default();
        m.record("a", 10, 5, 7);
        m.record("a", 20, 1, 2);
        m.record("b", 1, 1, 1);
        assert_eq!(
            m.usage("a"),
            Usage {
                calls: 2,
                fuel: 30,
                input_bytes: 6,
                output_bytes: 9
            }
        );
        assert_eq!(m.usage("b").calls, 1);
        assert_eq!(m.usage("none"), Usage::default());
        assert_eq!(
            m.snapshot()
                .iter()
                .map(|(n, _)| n.as_str())
                .collect::<Vec<_>>(),
            ["a", "b"]
        );
    }

    #[test]
    fn tenants_past_the_cap_share_one_entry() {
        let m = Meter::default();
        for i in 0..MAX_TENANTS {
            m.record(&format!("t{i}"), 1, 1, 1);
        }
        m.record("late", 4, 4, 4);
        m.record("later", 6, 6, 6);
        m.record("t0", 2, 2, 2);
        assert_eq!(m.snapshot().len(), MAX_TENANTS + 1);
        assert_eq!(m.usage(OTHER).calls, 2);
        assert_eq!(m.usage(OTHER).fuel, 10);
        assert_eq!(
            m.usage("t0").calls,
            2,
            "a tenant already counted keeps its entry"
        );
    }
}
