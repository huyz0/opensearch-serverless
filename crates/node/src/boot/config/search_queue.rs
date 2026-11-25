//! The `OSS_SEARCH_QUEUE_*` variables and `OSS_HTTP_MAX_CONNECTIONS`: the
//! bounded queue that holds a tenant's searches over its share of the
//! workers off the REST workers (ADR 0056).

use super::write_limit::Bounded;
use crate::search_queue::Config;

/// The REST server's workers.
fn workers() -> usize {
    oss_rest::ServerConfig::default().workers
}

/// Reads the connections and the queue; with no queue size nothing else is
/// checked. `slots` is the search gate's slot count: the two are
/// alternatives.
pub(in crate::boot) fn read(bounded: Bounded<'_>, slots: usize) -> Result<Config, String> {
    let count = |v: u64| usize::try_from(v).unwrap_or(usize::MAX);
    let workers = workers();
    let connections = bounded(
        "OSS_HTTP_MAX_CONNECTIONS",
        &oss_rest::ServerConfig::default()
            .max_connections
            .to_string(),
        workers as u64,
        1_000_000,
    )?;
    let size = count(bounded("OSS_SEARCH_QUEUE_SIZE", "0", 0, 1_000_000)?);
    let config = Config {
        size,
        share_pct: count(bounded("OSS_SEARCH_TENANT_SHARE_PCT", "25", 1, 100)?),
        reserved: count(bounded("OSS_SEARCH_RESERVED_WORKERS", "8", 0, 10_000)?),
        wait_millis: bounded("OSS_SEARCH_QUEUE_WAIT_MS", "30000", 1, 600_000)?,
        max_connections: connections,
    };
    if size > 0 {
        if config.reserved >= workers {
            return Err(format!(
                "OSS_SEARCH_RESERVED_WORKERS [{}] must be below the {workers} workers",
                config.reserved
            ));
        }
        let waiting = count(connections).saturating_sub(workers);
        if size > waiting {
            return Err(format!(
                "OSS_SEARCH_QUEUE_SIZE [{size}] is more than the {waiting} connections that may wait (OSS_HTTP_MAX_CONNECTIONS [{connections}] less {workers} workers)"
            ));
        }
        if slots > 0 {
            return Err(
                "OSS_SEARCH_QUEUE_SIZE and OSS_SEARCH_SLOTS are alternatives: set one".to_owned(),
            );
        }
    }
    Ok(config)
}
