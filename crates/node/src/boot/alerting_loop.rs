//! The thread that runs the alerting monitors this node tries.

use std::sync::Arc;
use std::time::Duration;

use oss_blob::BlobStore;
use oss_cluster::lease::{LeaseClock, Membership};

use super::renew::Renewal;
use crate::ad_route::Timing;
use crate::alerting_route::AlertingService;
use crate::alerting_run::Scheduler;
use crate::assemble::Assembled;

/// How often the thread asks the scheduler for work. A tick with nothing due
/// makes no request, so this bounds how late a slot starts and nothing else.
const EVERY: Duration = Duration::from_secs(5);

/// How long the scheduler trusts what it read of the registry: its refresh
/// is the 256 shards and a tenant's index each, so a node pays them once a
/// minute at most.
const REFRESH_MS: i64 = 60_000;

/// Starts the loop; `None` when it cannot start.
pub(super) fn run_monitors(
    boot: &super::NodeConfig,
    assembled: &Assembled,
    (store, membership, clock): (&Arc<dyn BlobStore>, &Arc<Membership>, &Arc<dyn LeaseClock>),
) -> Option<Renewal> {
    let mut service = AlertingService::new(store, &boot.base)
        .with_search(Arc::clone(&assembled.search) as _)
        .with_history(Arc::clone(&assembled.bulk) as _);
    if let Some(notifier) = &assembled.notifier {
        service = service.with_sender(Arc::clone(notifier) as _);
    }
    let scheduler = Scheduler::new(
        store,
        &boot.base,
        service,
        &boot.node_id,
        REFRESH_MS,
        Timing {
            max_skew_ms: 5_000,
            visibility_lag_ms: 5_000,
        },
    )
    .ok()?;
    let (members, clock) = (Arc::clone(membership), Arc::clone(clock));
    Renewal::spawn_named("alerting-monitors", EVERY, move || {
        let live: Vec<String> = members
            .current()
            .iter()
            .map(|lease| lease.node_id.clone())
            .collect();
        let refs: Vec<&str> = live.iter().map(String::as_str).collect();
        // A failed tick is tried again at the next one.
        let _ = scheduler.tick(clock.now_millis(), &refs);
    })
    .ok()
}
