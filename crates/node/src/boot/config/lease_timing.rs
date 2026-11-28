//! How often the lease is renewed and the fleet's leases read.

use std::time::Duration;

/// `OSS_LEASE_TTL_MS`: how long a lease lives, at least 3 milliseconds.
pub(super) fn ttl_millis(get: &impl Fn(&str) -> Option<String>) -> Result<i64, String> {
    let text = get("OSS_LEASE_TTL_MS")
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "30000".to_owned());
    let ttl: i64 = text.parse().map_err(|e| format!("OSS_LEASE_TTL_MS: {e}"))?;
    if ttl < 3 {
        return Err(format!("OSS_LEASE_TTL_MS [{ttl}] is below 3"));
    }
    Ok(ttl)
}

/// `OSS_LEASE_REFRESH_MS`: how often the fleet's leases are read, when set; at least 1.
pub(super) fn refresh_millis(get: &impl Fn(&str) -> Option<String>) -> Result<Option<u64>, String> {
    let Some(v) = get("OSS_LEASE_REFRESH_MS").filter(|v| !v.is_empty()) else {
        return Ok(None);
    };
    let millis: u64 = v
        .parse()
        .map_err(|e| format!("OSS_LEASE_REFRESH_MS: {e}"))?;
    if millis == 0 {
        return Err("OSS_LEASE_REFRESH_MS is below 1".to_owned());
    }
    Ok(Some(millis))
}

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
