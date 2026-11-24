//! A cap on the shards this node holds, and making room under it.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use oss_types::settings::{BoolSetting, LongSetting, Scope, Setting};

use oss_cluster::placement::{FleetView, HAND_OFF_DOWN_TO_MEAN, Holding};
use oss_format::NodeLease;

use super::{Key, OpenRefused, ShardSlot, WriterRole, lock, now_millis};
use oss_writer::bulk::IndexInfo;
use std::sync::{Arc, Mutex};

/// How far below its cap a node handing off lets go to, as a share of it.
const HAND_OFF_DOWN_TO: f64 = 0.8;

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn floor_of(cap: usize, share: f64) -> usize {
    (cap as f64 * share).floor() as usize
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn ceil_of(value: f64) -> usize {
    value.max(0.0).ceil() as usize
}

/// `serverless.activation.max_shards`: how many shards this node holds
/// before it makes room or refuses. Default 0, no cap.
pub const MAX_SHARDS: LongSetting = LongSetting::new(
    "serverless.activation.max_shards",
    0,
    0,
    1_000_000,
    Scope::Node,
    "How many writer shards this node holds before it lets an idle one go or refuses a new one. \
     0 holds as many as are asked for.",
);

/// `serverless.activation.evict_after_millis`: how long a held shard must be
/// unused before a node at its cap lets it go.
pub const EVICT_AFTER_MILLIS: LongSetting = LongSetting::new(
    "serverless.activation.evict_after_millis",
    60_000,
    0,
    604_800_000,
    Scope::Node,
    "How long a writer shard must be unused before a node at its shard cap lets it go to open \
     another, in milliseconds. 0 never lets one go: the node at its cap refuses.",
);

/// `serverless.activation.hand_off.enabled`: hand idle shards to a member
/// with room. Default false.
pub const HAND_OFF_ENABLED: BoolSetting = BoolSetting::new(
    "serverless.activation.hand_off.enabled",
    false,
    Scope::Node,
    "Whether a writer node nearly at its shard cap, or far above the fleet's mean, lets go of \
     idle shards so a member with room takes them on its next write. Off until measured to cost \
     nothing it saves.",
);

/// Every setting this module defines, for the node's registry.
pub static SETTINGS: &[&Setting] = &[
    MAX_SHARDS.setting(),
    EVICT_AFTER_MILLIS.setting(),
    HAND_OFF_ENABLED.setting(),
];

/// The cap on held shards and when one may be given up for another.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShardCap {
    /// Shards held at most; 0 is no cap.
    pub max: usize,
    /// How long a shard must be unused to be given up; 0 never.
    pub evict_after_millis: u64,
}

/// How full the node is and what fullness has cost.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShardCapStats {
    /// The cap; 0 is none.
    pub max: usize,
    /// Shards held now.
    pub held: usize,
    /// Opens refused at the cap.
    pub refused: u64,
    /// Shards given up to make room.
    pub evicted_for_room: u64,
    /// Shards handed to a member with room.
    pub handed_off: u64,
}

/// What a role counts at its cap.
#[derive(Default)]
pub(super) struct Counters {
    refused: AtomicU64,
    evicted: AtomicU64,
    handed_off: AtomicU64,
    /// Whether idle shards are handed to a member with room.
    hand_off: AtomicBool,
}

impl WriterRole {
    /// Caps the shards this role holds. False when a cap is set already.
    pub fn set_shard_cap(&self, cap: ShardCap) -> bool {
        self.cap.set(cap).is_ok()
    }

    /// The cap, the shards held, and the refusals and evictions so far.
    pub fn shard_cap_stats(&self) -> ShardCapStats {
        ShardCapStats {
            max: self.cap.get().map_or(0, |c| c.max),
            held: lock(&self.shards).len(),
            refused: self.counters.refused.load(Ordering::Relaxed),
            evicted_for_room: self.counters.evicted.load(Ordering::Relaxed),
            handed_off: self.counters.handed_off.load(Ordering::Relaxed),
        }
    }

    /// Whether this node hands idle shards to a member with room.
    pub fn set_hand_off(&self, on: bool) {
        self.counters.hand_off.store(on, Ordering::Relaxed);
    }

    /// Hands idle shards off as the fleet's leases `members` say at
    /// `lease_now` (their clock) with lease lifetime `ttl_millis`; how many
    /// were let go. Nearly full with a member that has room: down to
    /// `HAND_OFF_DOWN_TO` of the cap. Far above the fleet's mean with a
    /// member below it: down to [`HAND_OFF_DOWN_TO_MEAN`] times the mean. At
    /// most a tenth of the cap, and at least one, a pass. Nothing with the
    /// setting off, no cap, or no eviction grace.
    pub fn hand_off(
        &self,
        members: &[NodeLease],
        lease_now: i64,
        ttl_millis: i64,
        local: &str,
    ) -> usize {
        let Some(cap) = self
            .cap
            .get()
            .filter(|c| c.max > 0 && c.evict_after_millis > 0)
        else {
            return 0;
        };
        if !self.counters.hand_off.load(Ordering::Relaxed) {
            return 0;
        }
        let held = lock(&self.shards).len();
        let view = FleetView {
            members,
            now: lease_now,
            ttl: ttl_millis,
            local,
            holding: Holding {
                held: i32::try_from(held).unwrap_or(i32::MAX),
                cap: i32::try_from(cap.max).unwrap_or(i32::MAX),
            },
        };
        let floor = if view.nearly_full() && view.member_with_room().is_some() {
            floor_of(cap.max, HAND_OFF_DOWN_TO)
        } else if view.above_fleet_share() && view.member_below_mean().is_some() {
            ceil_of(view.fleet_mean_held() * HAND_OFF_DOWN_TO_MEAN)
        } else {
            return 0;
        };
        let most = (cap.max / 10).max(1);
        let mut handed = 0usize;
        while handed < most && lock(&self.shards).len() > floor {
            if !self.evict_one(now_millis(), true) {
                break;
            }
            handed = handed.saturating_add(1);
        }
        handed
    }

    /// Room for one more shard under the cap, made by letting idle shards
    /// go; refused when none can. No cap is always room, and no request.
    pub(super) fn admit_open(&self, index: &IndexInfo, shard: u32) -> Result<(), OpenRefused> {
        let Some(cap) = self.cap.get().filter(|c| c.max > 0) else {
            return Ok(());
        };
        while lock(&self.shards).len() >= cap.max {
            if self.make_room(now_millis()) == 0 {
                self.counters.refused.fetch_add(1, Ordering::Relaxed);
                return Err(OpenRefused::from(format!(
                    // ALLOC: error path; one message.
                    "[{}][{shard}]: this node is at its shard cap of {}",
                    index.name, cap.max
                )));
            }
        }
        Ok(())
    }

    /// Lets go of the shard unused longest, if it has been unused for the
    /// cap's `evict_after_millis` at `now_millis`; how many were let go.
    /// Its commit is published first when writes are unpublished, it closes
    /// only if nothing reached it meanwhile, and its head is given back; a
    /// head that cannot be given back stays this node's, the shard asleep.
    pub fn make_room(&self, now_millis: u64) -> usize {
        usize::from(self.evict_one(now_millis, false))
    }

    /// [`Self::make_room`] for one shard, counted as handed off when
    /// `handing_off` and as evicted for room otherwise.
    fn evict_one(&self, now_millis: u64, handing_off: bool) -> bool {
        let Some(cap) = self.cap.get().filter(|c| c.evict_after_millis > 0) else {
            return false;
        };
        let Some((key, slot, used)) = self.idlest(now_millis, cap.evict_after_millis) else {
            return false;
        };
        let Some(index) = self.names.name_of(&key.0) else {
            return false;
        };
        if let Some(publishing) = &self.publishing {
            let max_seq_no = lock(&slot).max_seq_no();
            if publishing.unpublished(&key, max_seq_no) {
                match self.publish_shard(publishing, &key, &index, &slot, false) {
                    Ok(Some((done, max_seq_no))) => {
                        if let Some(t) = self.tail.get() {
                            t.published(&key.0, key.1, max_seq_no);
                        }
                        self.staged(&key.0, key.1, &done.manifest);
                    }
                    Ok(None) => {}
                    Err(_) => return false,
                }
            }
        }
        let mut held = lock(&slot);
        if held.asleep()
            || held.released()
            || held.used_at() != used
            || self
                .publishing
                .as_ref()
                .is_some_and(|p| p.unpublished(&key, held.max_seq_no()))
        {
            return false;
        }
        held.sleep();
        let term = held.term();
        drop(held);
        // Recorded before the shard leaves the map, as hibernation does, so
        // a write arriving meanwhile finds it asleep and wakes it.
        self.remember_asleep(&key, term, now_millis);
        self.release(&key.0, key.1, &slot);
        let counter = if handing_off {
            &self.counters.handed_off
        } else {
            &self.counters.evicted
        };
        counter.fetch_add(1, Ordering::Relaxed);
        let given = self
            .opener
            .as_ref()
            .is_some_and(|o| o.give_back(&index, &key.0, key.1, term));
        if given {
            // Given back: the record would wake a head that is no longer ours.
            self.take_asleep(&key.0, key.1);
        }
        true
    }

    /// The held shard unused longest, if unused for `after_millis`.
    fn idlest(
        &self,
        now_millis: u64,
        after_millis: u64,
    ) -> Option<(Key, Arc<Mutex<ShardSlot>>, u64)> {
        // ALLOC: one handle a held shard, once an eviction.
        let held: Vec<(Key, Arc<Mutex<ShardSlot>>)> = lock(&self.shards)
            .iter()
            .map(|(k, v)| (k.clone(), Arc::clone(v)))
            .collect();
        held.into_iter()
            .filter_map(|(key, slot)| {
                let used = {
                    let s = lock(&slot);
                    if s.asleep() || s.released() {
                        return None;
                    }
                    s.used_at()
                };
                (now_millis.saturating_sub(used) >= after_millis).then_some((key, slot, used))
            })
            .min_by_key(|(_, _, used)| *used)
    }
}
