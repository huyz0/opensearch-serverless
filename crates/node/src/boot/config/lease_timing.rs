//! How often the lease is renewed and the fleet's leases read.

use std::time::Duration;

impl super::NodeConfig {
    /// How often the lease is renewed: a third of its lifetime.
    pub fn renew_every(&self) -> Duration {
        Duration::from_millis(u64::try_from(self.lease_ttl_millis / 3).unwrap_or(1).max(1))
    }

    /// How often the fleet's leases are read again: `OSS_LEASE_REFRESH_MS`,
    /// or as often as the lease is renewed.
    pub fn refresh_every(&self) -> Duration {
        self.lease_refresh_millis
            .map_or_else(|| self.renew_every(), Duration::from_millis)
    }
}
