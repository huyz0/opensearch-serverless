//! Keeping the collector's sidecar and the writer's ledger in step.
//!
//! The writer keeps a ledger: each epoch's `high`, reserved ahead in blocks
//! of [`RESERVE_AHEAD`], and `collected_below`, under which every pack is
//! gone. The collector keeps a sidecar of the ordinals it has collected, so
//! a restart re-reads only the packs left. Each pass the sidecar is built
//! up from the ledger, and after judging the ledger is advanced over the
//! ordinals the sidecar shows collected. The last reserved block of the
//! newest epoch is not judged while the node may be alive: an ordinal there
//! may be reserved and not yet written.

use oss_format::{ORDINALS_PER_EPOCH, PackEpoch, PackSidecar, SidecarEpoch};

use super::RESERVE_AHEAD;

/// `current` with every ledger epoch that still has packs to judge added, or
/// its `high` raised, and every epoch but the newest with nothing left to
/// judge dropped. `alive` is whether the node's lease is live: a dead node's
/// newest epoch is judged to its full `high`.
pub(super) fn synced(ledger: &[PackEpoch], current: &PackSidecar, alive: bool) -> PackSidecar {
    let newest = ledger.iter().map(|e| e.epoch).max();
    // ALLOC: the sidecar's epochs, at most one a ledger epoch.
    let mut epochs = current.epochs().to_vec();
    for l in ledger {
        let reserved = if alive && Some(l.epoch) == newest {
            l.high.saturating_sub(RESERVE_AHEAD)
        } else {
            l.high
        };
        let high = reserved.min(ORDINALS_PER_EPOCH);
        if l.collected_below >= high {
            continue;
        }
        match epochs.iter_mut().find(|e| e.epoch == l.epoch) {
            Some(e) => e.high = e.high.max(high),
            None => {
                let mut added = SidecarEpoch::new(l.epoch, high);
                for ordinal in 0..l.collected_below.min(ORDINALS_PER_EPOCH) {
                    added.mark(ordinal);
                }
                epochs.push(added);
            }
        }
    }
    epochs.sort_by_key(|e| e.epoch);
    let last = epochs.last().map(|e| e.epoch);
    epochs.retain(|e| Some(e.epoch) == last || e.unmarked().next().is_some());
    PackSidecar::new(epochs)
}

/// The ledger's epochs with `collected_below` moved up over the contiguous
/// ordinals `sidecar` shows collected, and every epoch but the newest that
/// is collected through its `high` dropped.
pub(super) fn advanced(ledger: &[PackEpoch], sidecar: &PackSidecar) -> Vec<PackEpoch> {
    let newest = ledger.iter().map(|e| e.epoch).max();
    ledger
        .iter()
        .filter_map(|l| {
            let mut below = l.collected_below;
            if let Some(s) = sidecar.epochs().iter().find(|s| s.epoch == l.epoch) {
                let limit = s.high.min(ORDINALS_PER_EPOCH).min(l.high);
                let run = (below..limit).take_while(|o| s.is_marked(*o)).count();
                below = below.saturating_add(u64::try_from(run).unwrap_or(0));
            }
            if below >= l.high && Some(l.epoch) != newest {
                None
            } else {
                Some(PackEpoch {
                    collected_below: below,
                    ..*l
                })
            }
        })
        // ALLOC: the ledger's epochs, at most the 1,500 a node keeps.
        .collect()
}

/// Whether `sidecar` still has a pack that may exist and is not judged.
pub(super) fn has_live(sidecar: &PackSidecar) -> bool {
    sidecar
        .epochs()
        .iter()
        .any(|e| e.unmarked().next().is_some())
}
