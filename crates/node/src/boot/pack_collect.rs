//! The thread that runs the node's pack collection pass.

use std::sync::Arc;
use std::time::Duration;

use oss_blob::BlobStore;
use oss_cluster::lease::{LeaseClock, Membership};
use oss_meta::PointInTimeStore;

use super::renew::Renewal;
use crate::pack_holds::HoldLimits;
use crate::pack_pass::{PackPass, PackPassParts};

/// How often the thread asks whether this node is elected for the window:
/// the judge's sighting gap, so a second sighting is never late by more.
const EVERY: Duration = Duration::from_secs(60);

/// The most packs one node's judge tracks.
const MAX_TRACKED: usize = 100_000;

/// What a pass reads of the views.
const LIMITS: HoldLimits = HoldLimits {
    max_records: 10_000,
    max_pins: 100_000,
    record_bytes: 1 << 16,
};

/// Starts the pass when the node packs with collection on; `None` otherwise,
/// and when the thread cannot start.
pub(super) fn start(
    node: &crate::assemble::Config,
    boot: &super::NodeConfig,
    store: &Arc<dyn BlobStore>,
    membership: &Arc<Membership>,
    clock: &Arc<dyn LeaseClock>,
) -> Option<Renewal> {
    if !node.node_packs.as_ref().is_some_and(|p| p.collect) {
        return None;
    }
    let (members, ticking, pit_clock) =
        (Arc::clone(membership), Arc::clone(clock), Arc::clone(clock));
    spawn(
        Parts {
            store: Arc::clone(store),
            base: boot.base.clone(),
            node_id: boot.node_id.clone(),
        },
        EVERY,
        Box::new(move || {
            members
                .current()
                .iter()
                .map(|lease| lease.node_id.clone())
                .collect()
        }),
        Arc::new(move || u64::try_from(ticking.now_millis()).unwrap_or(0)),
        pit_clock,
    )
}

/// Where the pass runs.
struct Parts {
    store: Arc<dyn BlobStore>,
    base: String,
    node_id: String,
}

/// The pass on a thread of its own, every `every`.
fn spawn(
    parts: Parts,
    every: Duration,
    members: Box<dyn Fn() -> Vec<String> + Send + Sync>,
    now: Arc<dyn Fn() -> u64 + Send + Sync>,
    pit_clock: Arc<dyn LeaseClock>,
) -> Option<Renewal> {
    let pits = PointInTimeStore::new(
        Arc::clone(&parts.store),
        &parts.base,
        Arc::new(move || u64::try_from(pit_clock.now_millis()).unwrap_or(0)),
    )
    .ok()?;
    let pass = PackPass::new(PackPassParts {
        store: parts.store,
        base: parts.base,
        node_id: parts.node_id,
        pits: Arc::new(pits),
        limits: LIMITS,
        max_tracked: MAX_TRACKED,
        members,
        now: Box::new(move || now()),
    });
    Renewal::spawn_named("pack-collect", every, move || {
        // A failed pass is tried again at the next tick.
        let _ = pass.run();
    })
    .ok()
}

#[cfg(test)]
#[path = "pack_collect_tests.rs"]
mod tests;
