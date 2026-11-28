//! The thread that runs the anomaly detectors this node owns.

use std::sync::Arc;
use std::time::Duration;

use oss_blob::BlobStore;
use oss_cluster::lease::{LeaseClock, Membership};

use super::renew::Renewal;
use crate::ad_route::{Runner, Scheduler, Timing};
use crate::assemble::Assembled;

/// How often the thread asks the scheduler for work. A tick with nothing due
/// makes no request, so this bounds how late a slot starts and nothing else.
const EVERY: Duration = Duration::from_secs(5);

/// How long the scheduler trusts what it read of the registry: its refresh
/// is 257 reads, so a node pays them once a minute at most.
const REFRESH_MS: i64 = 60_000;

/// Starts the loop when anomaly detection is on; `None` otherwise, and when
/// the loop cannot start.
pub(super) fn run_detectors(
    node: &crate::assemble::Config,
    boot: &super::NodeConfig,
    assembled: &Assembled,
    (store, membership, clock): (&Arc<dyn BlobStore>, &Arc<Membership>, &Arc<dyn LeaseClock>),
) -> Option<Renewal> {
    if !node.ad {
        return None;
    }
    let runner = Runner::new(
        store,
        &boot.base,
        Arc::clone(&assembled.search) as _,
        Arc::clone(&assembled.bulk) as _,
        Timing::DEFAULT,
    )
    .ok()?;
    let scheduler = Scheduler::new(
        store,
        &boot.base,
        runner,
        &boot.node_id,
        REFRESH_MS,
        node.pit_buffer_bytes,
    )
    .ok()?;
    let (members, clock) = (Arc::clone(membership), Arc::clone(clock));
    Renewal::spawn_named("ad-detectors", EVERY, move || {
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
